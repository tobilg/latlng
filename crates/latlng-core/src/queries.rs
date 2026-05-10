use super::*;

impl<P: Platform, S: StorageBackend> LatLng<P, S> {
    pub fn nearby(&self, collection: &str, query: NearbyQuery) -> Result<SearchResults> {
        let _gate = self.read_control();
        let handle = self.existing_collection_handle(collection)?;
        if query.options.fast_limited_ids() && !query.options.has_filters() {
            let collection_state = P::read(&*handle);
            return fast_ids_from_spatial_visit(
                &collection_state.collection,
                query.options.limit,
                |visit| {
                    collection_state.collection.spatial_index.visit_nearby_ids(
                        query.lat,
                        query.lon,
                        query.meters,
                        |id| visit(id),
                    );
                },
                |_| Ok(true),
            );
        }
        #[cfg(feature = "parallel")]
        {
            let collection_state = P::read(&*handle);
            let ids = collection_state.collection.spatial_index.nearby_ids(
                query.lat,
                query.lon,
                query.meters,
            );
            let objects = ids
                .iter()
                .filter_map(|id| collection_state.collection.objects.get(id))
                .filter(|object| !is_expired(object))
                .cloned()
                .collect::<Vec<_>>();
            let candidates = nearby_candidates_owned(objects, query.lat, query.lon);
            apply_search_options(candidates, &query.options, None, false).map_err(Into::into)
        }
        #[cfg(not(feature = "parallel"))]
        {
            let collection_state = P::read(&*handle);
            let ids = collection_state.collection.spatial_index.nearby_ids(
                query.lat,
                query.lon,
                query.meters,
            );
            let candidates = ids
                .iter()
                .filter_map(|id| collection_state.collection.objects.get(id))
                .filter(|object| !is_expired(object))
                .collect::<Vec<_>>();
            apply_search_options(
                nearby_candidates(candidates.into_iter(), query.lat, query.lon),
                &query.options,
                None,
                false,
            )
            .map_err(Into::into)
        }
    }

    pub fn within(
        &self,
        collection: &str,
        area: Area,
        options: SearchOptions,
    ) -> Result<SearchResults> {
        let _gate = self.read_control();
        let resolved_area = self.resolve_area(area)?;
        let handle = self.existing_collection_handle(collection)?;
        if options.fast_limited_ids() && !options.has_filters() {
            let collection_state = P::read(&*handle);
            let bounds = resolved_area.envelope()?;
            return fast_ids_from_spatial_visit(
                &collection_state.collection,
                options.limit,
                |visit| {
                    collection_state
                        .collection
                        .spatial_index
                        .visit_within_candidate_ids(bounds, |id| visit(id));
                },
                |object| resolved_area.contains_geo(&object.geo).map_err(Into::into),
            );
        }
        #[cfg(feature = "parallel")]
        {
            let collection_state = P::read(&*handle);
            let ids = collection_state
                .collection
                .spatial_index
                .within_candidate_ids(resolved_area.envelope()?);
            let objects = ids
                .iter()
                .filter_map(|id| collection_state.collection.objects.get(id))
                .filter(|object| !is_expired(object))
                .cloned()
                .collect::<Vec<_>>();
            let candidates = area_candidates_owned(objects, &resolved_area, AreaPredicate::Within)?;
            apply_search_options(candidates, &options, Some(&resolved_area), false)
                .map_err(Into::into)
        }
        #[cfg(not(feature = "parallel"))]
        {
            let collection_state = P::read(&*handle);
            let ids = collection_state
                .collection
                .spatial_index
                .within_candidate_ids(resolved_area.envelope()?);
            let candidates = ids
                .iter()
                .filter_map(|id| collection_state.collection.objects.get(id))
                .filter(|object| !is_expired(object))
                .filter_map(|object| {
                    resolved_area
                        .contains_geo(&object.geo)
                        .ok()
                        .filter(|contains| *contains)
                        .map(|_| SearchCandidate {
                            object,
                            distance_meters: None,
                        })
                })
                .collect::<Vec<_>>();
            apply_search_options(candidates, &options, Some(&resolved_area), false)
                .map_err(Into::into)
        }
    }

    pub fn intersects(
        &self,
        collection: &str,
        area: Area,
        options: SearchOptions,
    ) -> Result<SearchResults> {
        let _gate = self.read_control();
        let resolved_area = self.resolve_area(area)?;
        let handle = self.existing_collection_handle(collection)?;
        if options.fast_limited_ids() && !options.has_filters() {
            let collection_state = P::read(&*handle);
            let bounds = resolved_area.envelope()?;
            return fast_ids_from_spatial_visit(
                &collection_state.collection,
                options.limit,
                |visit| {
                    collection_state
                        .collection
                        .spatial_index
                        .visit_intersecting_candidate_ids(bounds, |id| visit(id));
                },
                |object| {
                    resolved_area
                        .intersects_geo(&object.geo)
                        .map_err(Into::into)
                },
            );
        }
        #[cfg(feature = "parallel")]
        {
            let collection_state = P::read(&*handle);
            let ids = collection_state
                .collection
                .spatial_index
                .intersecting_candidate_ids(resolved_area.envelope()?);
            let objects = ids
                .iter()
                .filter_map(|id| collection_state.collection.objects.get(id))
                .filter(|object| !is_expired(object))
                .cloned()
                .collect::<Vec<_>>();
            let candidates =
                area_candidates_owned(objects, &resolved_area, AreaPredicate::Intersects)?;
            apply_search_options(candidates, &options, Some(&resolved_area), true)
                .map_err(Into::into)
        }
        #[cfg(not(feature = "parallel"))]
        {
            let collection_state = P::read(&*handle);
            let ids = collection_state
                .collection
                .spatial_index
                .intersecting_candidate_ids(resolved_area.envelope()?);
            let candidates = ids
                .iter()
                .filter_map(|id| collection_state.collection.objects.get(id))
                .filter(|object| !is_expired(object))
                .filter_map(|object| {
                    resolved_area
                        .intersects_geo(&object.geo)
                        .ok()
                        .filter(|intersects| *intersects)
                        .map(|_| SearchCandidate {
                            object,
                            distance_meters: None,
                        })
                })
                .collect::<Vec<_>>();
            apply_search_options(candidates, &options, Some(&resolved_area), true)
                .map_err(Into::into)
        }
    }

    pub fn scan(&self, collection: &str, options: SearchOptions) -> Result<SearchResults> {
        let _gate = self.read_control();
        let handle = self.existing_collection_handle(collection)?;
        {
            let collection_state = P::read(&*handle);
            if let Some(results) = collection_state
                .collection
                .fast_indexed_results(&options, false)?
            {
                return Ok(results);
            }
        }
        #[cfg(feature = "parallel")]
        {
            let collection_state = P::read(&*handle);
            let objects =
                if let Some(ids) = collection_state.collection.indexed_candidate_ids(&options) {
                    ids.iter()
                        .filter_map(|id| collection_state.collection.objects.get(id))
                        .filter(|object| !is_expired(object))
                        .cloned()
                        .collect::<Vec<_>>()
                } else {
                    collection_state
                        .collection
                        .objects
                        .values()
                        .filter(|object| !is_expired(object))
                        .cloned()
                        .collect::<Vec<_>>()
                };
            let candidates = snapshot_candidates_owned(objects);
            apply_search_options(candidates, &options, None, false).map_err(Into::into)
        }
        #[cfg(not(feature = "parallel"))]
        {
            let collection_state = P::read(&*handle);
            let candidates =
                if let Some(ids) = collection_state.collection.indexed_candidate_ids(&options) {
                    ids.iter()
                        .filter_map(|id| collection_state.collection.objects.get(id))
                        .filter(|object| !is_expired(object))
                        .map(|object| SearchCandidate {
                            object,
                            distance_meters: None,
                        })
                        .collect::<Vec<_>>()
                } else {
                    collection_state
                        .collection
                        .objects
                        .values()
                        .filter(|object| !is_expired(object))
                        .map(|object| SearchCandidate {
                            object,
                            distance_meters: None,
                        })
                        .collect::<Vec<_>>()
                };
            apply_search_options(candidates, &options, None, false).map_err(Into::into)
        }
    }

    pub fn search(&self, collection: &str, options: SearchOptions) -> Result<SearchResults> {
        let _gate = self.read_control();
        let handle = self.existing_collection_handle(collection)?;
        {
            let collection_state = P::read(&*handle);
            if let Some(results) = collection_state
                .collection
                .fast_indexed_results(&options, true)?
            {
                return Ok(results);
            }
        }
        #[cfg(feature = "parallel")]
        {
            let collection_state = P::read(&*handle);
            let objects =
                if let Some(ids) = collection_state.collection.indexed_candidate_ids(&options) {
                    ids.iter()
                        .filter_map(|id| collection_state.collection.objects.get(id))
                        .filter(|object| !is_expired(object))
                        .cloned()
                        .collect::<Vec<_>>()
                } else {
                    collection_state
                        .collection
                        .objects
                        .values()
                        .filter(|object| !is_expired(object))
                        .cloned()
                        .collect::<Vec<_>>()
                };
            let candidates = string_snapshot_candidates_owned(objects);
            apply_search_options(candidates, &options, None, false).map_err(Into::into)
        }
        #[cfg(not(feature = "parallel"))]
        {
            let collection_state = P::read(&*handle);
            let candidates =
                if let Some(ids) = collection_state.collection.indexed_candidate_ids(&options) {
                    ids.iter()
                        .filter_map(|id| collection_state.collection.objects.get(id))
                        .filter(|object| !is_expired(object))
                        .filter(|object| matches!(object.geo, GeoType::String(_)))
                        .map(|object| SearchCandidate {
                            object,
                            distance_meters: None,
                        })
                        .collect::<Vec<_>>()
                } else {
                    collection_state
                        .collection
                        .objects
                        .values()
                        .filter(|object| !is_expired(object))
                        .filter(|object| matches!(object.geo, GeoType::String(_)))
                        .map(|object| SearchCandidate {
                            object,
                            distance_meters: None,
                        })
                        .collect::<Vec<_>>()
                };
            apply_search_options(candidates, &options, None, false).map_err(Into::into)
        }
    }

    pub fn test(&self, a: &GeoType, within: bool, b: &GeoType) -> Result<bool> {
        let _gate = self.read_control();
        let area = Area::GeoJson(b.to_geojson_value()?);
        if within {
            area.contains_geo(a).map_err(Into::into)
        } else {
            area.intersects_geo(a).map_err(Into::into)
        }
    }
}

fn fast_ids_from_spatial_visit(
    collection: &Collection,
    limit: u32,
    mut visit_ids: impl FnMut(&mut dyn FnMut(&str) -> bool),
    mut matches: impl FnMut(&Object) -> Result<bool>,
) -> Result<SearchResults> {
    let limit = limit.max(1) as usize;
    let mut results = Vec::with_capacity(limit);
    let mut error = None;
    visit_ids(&mut |id| {
        let Some(object) = collection.objects.get(id) else {
            return true;
        };
        let matched = match matches(object) {
            Ok(matched) => matched,
            Err(err) => {
                error = Some(err);
                return false;
            }
        };
        if is_expired(object) || !matched {
            return true;
        }
        results.push(SearchItem {
            id: object.id.clone(),
            object: None,
            fields: None,
            distance_meters: None,
        });
        results.len() < limit
    });
    if let Some(error) = error {
        return Err(error);
    }
    Ok(SearchResults {
        count: results.len() as u32,
        cursor: 0,
        results,
    })
}
