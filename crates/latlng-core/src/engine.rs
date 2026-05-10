use super::*;

impl<P: Platform, S: StorageBackend> LatLng<P, S> {
    pub fn builder() -> LatLngBuilder<P, S> {
        LatLngBuilder::default()
    }

    pub(crate) fn read_control<'a>(&'a self) -> P::ReadGuard<'a, ()> {
        P::read(&self.control)
    }

    pub(crate) fn write_control<'a>(&'a self) -> P::WriteGuard<'a, ()> {
        P::write(&self.control)
    }

    pub(crate) fn collection_handle(&self, collection: &str) -> Option<CollectionHandle<P>> {
        P::read(&self.collections).get(collection).cloned()
    }

    pub(crate) fn existing_collection_handle(
        &self,
        collection: &str,
    ) -> Result<CollectionHandle<P>> {
        self.collection_handle(collection)
            .ok_or_else(|| CoreError::CollectionNotFound(collection.to_owned()))
    }

    pub(crate) fn ensure_collection_cell(&self, collection: &str) -> CollectionHandle<P> {
        if let Some(handle) = self.collection_handle(collection) {
            return handle;
        }

        let mut collections = P::write(&self.collections);
        collections
            .entry(collection.to_owned())
            .or_insert_with(|| P::shared(P::new_rwlock(VersionedCollection::new(collection))))
            .clone()
    }

    pub(crate) fn collection_requires_exclusive_geofence_path(&self, collection: &str) -> bool {
        P::read(&self.geofences).requires_exclusive_roam_path(collection)
    }

    pub(crate) fn collection_has_geofence_side_effects(&self, collection: &str) -> bool {
        P::read(&self.geofences).has_relevant_side_effects(collection)
    }

    pub(crate) fn get_live_object_from_handle(
        &self,
        handle: &CollectionHandle<P>,
        id: &str,
    ) -> Option<Object> {
        let guard = P::read(&**handle);
        guard
            .collection
            .objects
            .get(id)
            .cloned()
            .filter(|object| !is_expired(object))
    }
}
