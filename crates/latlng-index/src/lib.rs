#![forbid(unsafe_code)]

use std::collections::HashMap;

use glob_match::glob_match;
use latlng_geo::{
    Area, BoundingBox, FieldValue, GeoError, GeoType, Object, clip_geometry_to_area,
    encode_geohash, geometry_to_geojson_value, get_json_path, haversine_distance_meters,
};
#[cfg(feature = "parallel")]
use rayon::prelude::*;
use regex::Regex;
use rstar::{AABB, PointDistance, RTree, RTreeObject};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use latlng_geo as geo;

#[derive(Debug, Error)]
pub enum IndexError {
    #[error(transparent)]
    Geo(#[from] GeoError),
    #[error("invalid regex: {0}")]
    Regex(String),
    #[error("unsupported clip operation for this geometry")]
    UnsupportedClip,
    #[error("invalid expression: {0}")]
    InvalidExpression(String),
}

pub type IndexResult<T> = Result<T, IndexError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortOrder {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum OutputFormat {
    #[default]
    Objects,
    Points,
    Bounds,
    Hashes {
        precision: u8,
    },
    Ids,
    Count,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WhereComparison {
    Range { min: f64, max: f64 },
    EqualsText(String),
    Regex(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WhereFilter {
    pub field: String,
    pub comparison: WhereComparison,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WhereInFilter {
    pub field: String,
    pub values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WhereExprFilter {
    pub expression: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchOptions {
    pub cursor: u32,
    pub limit: u32,
    pub nofields: bool,
    pub include_count: bool,
    pub match_pattern: Option<String>,
    pub sort: SortOrder,
    pub where_filters: Vec<WhereFilter>,
    pub where_in_filters: Vec<WhereInFilter>,
    pub where_expr_filters: Vec<WhereExprFilter>,
    pub clip: bool,
    pub output: OutputFormat,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            cursor: 0,
            limit: 100,
            nofields: false,
            include_count: true,
            match_pattern: None,
            sort: SortOrder::Asc,
            where_filters: Vec::new(),
            where_in_filters: Vec::new(),
            where_expr_filters: Vec::new(),
            clip: false,
            output: OutputFormat::Objects,
        }
    }
}

impl SearchOptions {
    pub fn fast_limited_ids(&self) -> bool {
        !self.include_count
            && self.cursor == 0
            && self.nofields
            && matches!(self.output, OutputFormat::Ids)
            && !self.clip
    }

    pub fn has_filters(&self) -> bool {
        self.match_pattern.is_some()
            || !self.where_filters.is_empty()
            || !self.where_in_filters.is_empty()
            || !self.where_expr_filters.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchItem {
    pub id: String,
    pub object: Option<GeoType>,
    pub fields: Option<latlng_geo::FieldMap>,
    pub distance_meters: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResults {
    pub results: Vec<SearchItem>,
    pub cursor: u32,
    pub count: u32,
}

#[derive(Debug, Clone)]
pub struct SearchCandidate<'a> {
    pub object: &'a Object,
    pub distance_meters: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OwnedSearchCandidate {
    pub object: Object,
    pub distance_meters: Option<f64>,
}

pub trait CandidateLike {
    fn object(&self) -> &Object;
    fn distance_meters(&self) -> Option<f64>;
}

impl CandidateLike for OwnedSearchCandidate {
    fn object(&self) -> &Object {
        &self.object
    }

    fn distance_meters(&self) -> Option<f64> {
        self.distance_meters
    }
}

impl CandidateLike for SearchCandidate<'_> {
    fn object(&self) -> &Object {
        self.object
    }

    fn distance_meters(&self) -> Option<f64> {
        self.distance_meters
    }
}

#[derive(Debug, Clone)]
pub struct SpatialIndex {
    tree: RTree<IndexedEntry>,
    entries_by_id: HashMap<String, IndexedEntry>,
}

#[derive(Debug, Clone, PartialEq)]
struct IndexedEntry {
    id: String,
    envelope: AABB<[f64; 2]>,
    center: [f64; 2],
}

impl IndexedEntry {
    fn new(id: String, bounds: BoundingBox) -> Self {
        let center = bounds.center();
        Self {
            id,
            envelope: AABB::from_corners(
                [bounds.min_lon, bounds.min_lat],
                [bounds.max_lon, bounds.max_lat],
            ),
            center: [center.1, center.0],
        }
    }
}

impl RTreeObject for IndexedEntry {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

impl PointDistance for IndexedEntry {
    fn distance_2(&self, point: &[f64; 2]) -> f64 {
        let dx = self.center[0] - point[0];
        let dy = self.center[1] - point[1];
        dx * dx + dy * dy
    }
}

impl Default for SpatialIndex {
    fn default() -> Self {
        Self {
            tree: RTree::new(),
            entries_by_id: HashMap::new(),
        }
    }
}

impl SpatialIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, id: impl Into<String>, geo: &GeoType) -> IndexResult<()> {
        let Some(bounds) = geo.envelope()? else {
            return Ok(());
        };
        let id = id.into();
        self.remove(&id);
        let entry = IndexedEntry::new(id.clone(), bounds);
        self.tree.insert(entry.clone());
        self.entries_by_id.insert(id, entry);
        Ok(())
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let Some(entry) = self.entries_by_id.remove(id) else {
            return false;
        };
        self.tree.remove(&entry).is_some()
    }

    pub fn nearby_ids(&self, lat: f64, lon: f64, max_distance_meters: f64) -> Vec<String> {
        let mut ids = Vec::new();
        self.visit_nearby_ids(lat, lon, max_distance_meters, |id| {
            ids.push(id.to_owned());
            true
        });
        ids
    }

    pub fn within_candidate_ids(&self, bounds: BoundingBox) -> Vec<String> {
        let mut ids = Vec::new();
        self.visit_within_candidate_ids(bounds, |id| {
            ids.push(id.to_owned());
            true
        });
        ids
    }

    pub fn intersecting_candidate_ids(&self, bounds: BoundingBox) -> Vec<String> {
        let mut ids = Vec::new();
        self.visit_intersecting_candidate_ids(bounds, |id| {
            ids.push(id.to_owned());
            true
        });
        ids
    }

    pub fn visit_nearby_ids(
        &self,
        lat: f64,
        lon: f64,
        max_distance_meters: f64,
        mut visit: impl FnMut(&str) -> bool,
    ) {
        for entry in self.tree.nearest_neighbor_iter(&[lon, lat]) {
            let distance = haversine_distance_meters(lat, lon, entry.center[1], entry.center[0]);
            if distance <= max_distance_meters && !visit(&entry.id) {
                break;
            }
        }
    }

    pub fn visit_within_candidate_ids(
        &self,
        bounds: BoundingBox,
        mut visit: impl FnMut(&str) -> bool,
    ) {
        for entry in self.tree.locate_in_envelope(&AABB::from_corners(
            [bounds.min_lon, bounds.min_lat],
            [bounds.max_lon, bounds.max_lat],
        )) {
            if !visit(&entry.id) {
                break;
            }
        }
    }

    pub fn visit_intersecting_candidate_ids(
        &self,
        bounds: BoundingBox,
        mut visit: impl FnMut(&str) -> bool,
    ) {
        for entry in self
            .tree
            .locate_in_envelope_intersecting(&AABB::from_corners(
                [bounds.min_lon, bounds.min_lat],
                [bounds.max_lon, bounds.max_lat],
            ))
        {
            if !visit(&entry.id) {
                break;
            }
        }
    }
}

pub fn nearby_candidates<'a>(
    objects: impl Iterator<Item = &'a Object>,
    lat: f64,
    lon: f64,
) -> Vec<SearchCandidate<'a>> {
    let mut items = objects
        .filter_map(|object| {
            candidate_distance(object, lat, lon).map(|distance| (object, distance))
        })
        .map(|(object, distance)| SearchCandidate {
            object,
            distance_meters: Some(distance),
        })
        .collect::<Vec<_>>();
    sort_candidates(&mut items);
    items
}

pub fn nearby_candidates_owned(
    objects: Vec<Object>,
    lat: f64,
    lon: f64,
) -> Vec<OwnedSearchCandidate> {
    #[cfg(feature = "parallel")]
    {
        nearby_candidates_owned_impl(objects, lat, lon, true)
    }
    #[cfg(not(feature = "parallel"))]
    {
        nearby_candidates_owned_impl(objects, lat, lon, false)
    }
}

pub fn area_candidates_owned(
    objects: Vec<Object>,
    area: &Area,
    predicate: AreaPredicate,
) -> IndexResult<Vec<OwnedSearchCandidate>> {
    #[cfg(feature = "parallel")]
    {
        area_candidates_owned_impl(objects, area, predicate, true)
    }
    #[cfg(not(feature = "parallel"))]
    {
        area_candidates_owned_impl(objects, area, predicate, false)
    }
}

pub fn snapshot_candidates_owned(objects: Vec<Object>) -> Vec<OwnedSearchCandidate> {
    objects
        .into_iter()
        .map(|object| OwnedSearchCandidate {
            object,
            distance_meters: None,
        })
        .collect()
}

pub fn string_snapshot_candidates_owned(objects: Vec<Object>) -> Vec<OwnedSearchCandidate> {
    objects
        .into_iter()
        .filter(|object| matches!(object.geo, GeoType::String(_)))
        .map(|object| OwnedSearchCandidate {
            object,
            distance_meters: None,
        })
        .collect()
}

pub fn apply_search_options<C>(
    candidates: Vec<C>,
    options: &SearchOptions,
    area: Option<&Area>,
    allow_clip: bool,
) -> IndexResult<SearchResults>
where
    C: CandidateLike + Send,
{
    if options.fast_limited_ids() {
        return apply_fast_limited_ids(candidates, options);
    }

    let mut filtered = filter_candidates(candidates, options)?;
    let total = filtered.len() as u32;
    let start = options.cursor as usize;
    let limit = options.limit.max(1) as usize;
    let end = start.saturating_add(limit).min(filtered.len());
    let next_cursor = if end < filtered.len() { end as u32 } else { 0 };

    if matches!(options.output, OutputFormat::Count) {
        return Ok(SearchResults {
            results: Vec::new(),
            cursor: next_cursor,
            count: total,
        });
    }

    sort_candidates_for_page(&mut filtered, options, end);

    let mut results = Vec::new();
    for candidate in filtered.into_iter().skip(start).take(limit) {
        let clip = allow_clip && options.clip && matches!(options.output, OutputFormat::Objects);
        let object = format_object(candidate.object(), options.output, area, clip)?;
        let fields = if options.nofields {
            None
        } else {
            Some(candidate.object().fields.clone())
        };
        results.push(SearchItem {
            id: candidate.object().id.clone(),
            object,
            fields,
            distance_meters: candidate.distance_meters(),
        });
    }

    Ok(SearchResults {
        results,
        cursor: next_cursor,
        count: if options.include_count {
            total
        } else {
            end.saturating_sub(start) as u32
        },
    })
}

fn apply_fast_limited_ids<C>(
    candidates: Vec<C>,
    options: &SearchOptions,
) -> IndexResult<SearchResults>
where
    C: CandidateLike + Send,
{
    let limit = options.limit.max(1) as usize;
    let mut results = Vec::with_capacity(limit);
    for candidate in candidates {
        if !matches_candidate(candidate.object(), options)? {
            continue;
        }
        results.push(SearchItem {
            id: candidate.object().id.clone(),
            object: None,
            fields: None,
            distance_meters: candidate.distance_meters(),
        });
        if results.len() >= limit {
            break;
        }
    }
    Ok(SearchResults {
        count: results.len() as u32,
        cursor: 0,
        results,
    })
}

#[cfg(feature = "parallel")]
fn filter_candidates<C>(candidates: Vec<C>, options: &SearchOptions) -> IndexResult<Vec<C>>
where
    C: CandidateLike + Send,
{
    if !parallel_filter_enabled(candidates.len()) {
        return filter_candidates_serial(candidates, options);
    }

    let filtered = candidates
        .into_par_iter()
        .map(|candidate| {
            matches_candidate(candidate.object(), options)
                .map(|matches| matches.then_some(candidate))
        })
        .collect::<Vec<_>>();

    let mut out = Vec::new();
    for item in filtered {
        if let Some(candidate) = item? {
            out.push(candidate);
        }
    }
    Ok(out)
}

fn filter_candidates_serial<C>(candidates: Vec<C>, options: &SearchOptions) -> IndexResult<Vec<C>>
where
    C: CandidateLike + Send,
{
    let mut filtered = Vec::new();
    for candidate in candidates {
        if !matches_candidate(candidate.object(), options)? {
            continue;
        }
        filtered.push(candidate);
    }
    Ok(filtered)
}

#[cfg(not(feature = "parallel"))]
fn filter_candidates<C>(candidates: Vec<C>, options: &SearchOptions) -> IndexResult<Vec<C>>
where
    C: CandidateLike + Send,
{
    filter_candidates_serial(candidates, options)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AreaPredicate {
    Within,
    Intersects,
}

#[cfg(feature = "parallel")]
const PARALLEL_DISTANCE_THRESHOLD: usize = 256;
#[cfg(feature = "parallel")]
const PARALLEL_GEOMETRY_THRESHOLD: usize = 128;
#[cfg(feature = "parallel")]
const PARALLEL_FILTER_THRESHOLD: usize = 128;
const PARTIAL_SORT_THRESHOLD: usize = 256;

fn sort_candidates<C: CandidateLike>(items: &mut [C]) {
    items.sort_by(|left, right| compare_candidates(left, right, false));
}

fn compare_candidates<C: CandidateLike>(
    left: &C,
    right: &C,
    descending: bool,
) -> std::cmp::Ordering {
    let ordering = match left.distance_meters().zip(right.distance_meters()) {
        Some((left_distance, right_distance)) => left_distance
            .partial_cmp(&right_distance)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.object().id.cmp(&right.object().id)),
        None => left.object().id.cmp(&right.object().id),
    };
    if descending {
        ordering.reverse()
    } else {
        ordering
    }
}

fn sort_candidates_for_page<C: CandidateLike>(
    items: &mut [C],
    options: &SearchOptions,
    end: usize,
) {
    let descending = matches!(options.sort, SortOrder::Desc);
    if items.len() <= 1 {
        return;
    }
    if end == 0 || end >= items.len() || !should_partial_sort(items.len(), end) {
        items.sort_by(|left, right| compare_candidates(left, right, descending));
        return;
    }

    let _ = items.select_nth_unstable_by(end - 1, |left, right| {
        compare_candidates(left, right, descending)
    });
    let prefix = &mut items[..end];
    prefix.sort_by(|left, right| compare_candidates(left, right, descending));
}

fn should_partial_sort(total: usize, end: usize) -> bool {
    total >= PARTIAL_SORT_THRESHOLD && end.saturating_mul(2) < total
}

#[cfg(feature = "parallel")]
fn parallel_filter_enabled(candidate_count: usize) -> bool {
    candidate_count >= PARALLEL_FILTER_THRESHOLD
}

fn nearby_candidates_owned_impl(
    objects: Vec<Object>,
    lat: f64,
    lon: f64,
    _parallel: bool,
) -> Vec<OwnedSearchCandidate> {
    #[cfg(feature = "parallel")]
    if _parallel && objects.len() >= PARALLEL_DISTANCE_THRESHOLD {
        let mut items = objects
            .into_par_iter()
            .filter_map(|object| {
                candidate_distance(&object, lat, lon).map(|distance| OwnedSearchCandidate {
                    object,
                    distance_meters: Some(distance),
                })
            })
            .collect::<Vec<_>>();
        sort_candidates(&mut items);
        return items;
    }

    let mut items = objects
        .into_iter()
        .filter_map(|object| {
            candidate_distance(&object, lat, lon).map(|distance| OwnedSearchCandidate {
                object,
                distance_meters: Some(distance),
            })
        })
        .collect::<Vec<_>>();
    sort_candidates(&mut items);
    items
}

fn area_candidates_owned_impl(
    objects: Vec<Object>,
    area: &Area,
    predicate: AreaPredicate,
    _parallel: bool,
) -> IndexResult<Vec<OwnedSearchCandidate>> {
    #[cfg(feature = "parallel")]
    if _parallel && objects.len() >= PARALLEL_GEOMETRY_THRESHOLD {
        let filtered = objects
            .into_par_iter()
            .map(|object| {
                let matched = match predicate {
                    AreaPredicate::Within => area.contains_geo(&object.geo),
                    AreaPredicate::Intersects => area.intersects_geo(&object.geo),
                }?;
                Ok(matched.then_some(OwnedSearchCandidate {
                    object,
                    distance_meters: None,
                }))
            })
            .collect::<Vec<IndexResult<Option<_>>>>();

        let mut out = Vec::new();
        for item in filtered {
            if let Some(candidate) = item? {
                out.push(candidate);
            }
        }
        return Ok(out);
    }

    let mut out = Vec::new();
    for object in objects {
        let matched = match predicate {
            AreaPredicate::Within => area.contains_geo(&object.geo)?,
            AreaPredicate::Intersects => area.intersects_geo(&object.geo)?,
        };
        if matched {
            out.push(OwnedSearchCandidate {
                object,
                distance_meters: None,
            });
        }
    }
    Ok(out)
}

pub fn candidate_distance(object: &Object, lat: f64, lon: f64) -> Option<f64> {
    if let Some((point_lat, point_lon)) = object.geo.point_coordinates() {
        return Some(haversine_distance_meters(lat, lon, point_lat, point_lon));
    }
    let envelope = object.envelope().ok().flatten()?;
    let (center_lat, center_lon) = envelope.center();
    Some(haversine_distance_meters(lat, lon, center_lat, center_lon))
}

pub fn matches_candidate(object: &Object, options: &SearchOptions) -> IndexResult<bool> {
    if let Some(pattern) = &options.match_pattern
        && !glob_match(pattern, &object.id)
    {
        return Ok(false);
    }

    for filter in &options.where_filters {
        if !matches_where(object, filter)? {
            return Ok(false);
        }
    }

    for filter in &options.where_in_filters {
        if !matches_where_in(object, filter) {
            return Ok(false);
        }
    }

    for filter in &options.where_expr_filters {
        if !evaluate_expression(object, &filter.expression)? {
            return Ok(false);
        }
    }

    Ok(true)
}

fn matches_where(object: &Object, filter: &WhereFilter) -> IndexResult<bool> {
    let value = resolve_value(object, &filter.field);
    Ok(match &filter.comparison {
        WhereComparison::Range { min, max } => value
            .and_then(|value| numeric_value(&value))
            .is_some_and(|number| number >= *min && number <= *max),
        WhereComparison::EqualsText(expected) => value
            .map(|value| string_value(&value) == *expected)
            .unwrap_or(false),
        WhereComparison::Regex(pattern) => {
            let regex =
                Regex::new(pattern).map_err(|error| IndexError::Regex(error.to_string()))?;
            value
                .map(|value| regex.is_match(&string_value(&value)))
                .unwrap_or(false)
        }
    })
}

fn matches_where_in(object: &Object, filter: &WhereInFilter) -> bool {
    resolve_value(object, &filter.field)
        .map(|value| {
            let rendered = string_value(&value);
            filter.values.iter().any(|candidate| candidate == &rendered)
        })
        .unwrap_or(false)
}

fn evaluate_expression(object: &Object, expression: &str) -> IndexResult<bool> {
    let disjunctions = expression
        .split("||")
        .map(str::trim)
        .filter(|item| !item.is_empty());

    let mut any_branch = false;
    for branch in disjunctions {
        any_branch = true;
        let mut branch_ok = true;
        for clause in branch
            .split("&&")
            .map(str::trim)
            .filter(|item| !item.is_empty())
        {
            if !evaluate_clause(object, clause)? {
                branch_ok = false;
                break;
            }
        }
        if branch_ok {
            return Ok(true);
        }
    }

    if any_branch {
        Ok(false)
    } else {
        Err(IndexError::InvalidExpression(expression.to_owned()))
    }
}

fn evaluate_clause(object: &Object, clause: &str) -> IndexResult<bool> {
    for operator in ["=~", ">=", "<=", "==", "!=", ">", "<"] {
        if let Some((left, right)) = clause.split_once(operator) {
            let left = left.trim();
            let right = right.trim();
            let value = resolve_value(object, left);
            return match operator {
                "=~" => {
                    let regex = Regex::new(&trim_quotes(right))
                        .map_err(|error| IndexError::Regex(error.to_string()))?;
                    Ok(value
                        .map(|value| regex.is_match(&string_value(&value)))
                        .unwrap_or(false))
                }
                "==" => Ok(value
                    .map(|value| compare_value(&value, right, |a, b| a == b))
                    .unwrap_or(false)),
                "!=" => Ok(value
                    .map(|value| compare_value(&value, right, |a, b| a != b))
                    .unwrap_or(false)),
                ">" => Ok(value
                    .and_then(|value| numeric_value(&value))
                    .is_some_and(|number| number > parse_number(right))),
                "<" => Ok(value
                    .and_then(|value| numeric_value(&value))
                    .is_some_and(|number| number < parse_number(right))),
                ">=" => Ok(value
                    .and_then(|value| numeric_value(&value))
                    .is_some_and(|number| number >= parse_number(right))),
                "<=" => Ok(value
                    .and_then(|value| numeric_value(&value))
                    .is_some_and(|number| number <= parse_number(right))),
                _ => Err(IndexError::InvalidExpression(clause.to_owned())),
            };
        }
    }

    Err(IndexError::InvalidExpression(clause.to_owned()))
}

fn compare_value(value: &ResolvedValue, right: &str, cmp: impl Fn(&str, &str) -> bool) -> bool {
    let rendered = string_value(value);
    cmp(&rendered, &trim_quotes(right))
}

fn trim_quotes(value: &str) -> String {
    value.trim().trim_matches('"').trim_matches('\'').to_owned()
}

fn parse_number(value: &str) -> f64 {
    match trim_quotes(value).as_str() {
        "+inf" | "inf" => f64::INFINITY,
        "-inf" => f64::NEG_INFINITY,
        other => other.parse::<f64>().unwrap_or(0.0),
    }
}

#[derive(Debug, Clone)]
enum ResolvedValue {
    Number(f64),
    Text(String),
    Json(serde_json::Value),
}

fn resolve_value(object: &Object, path: &str) -> Option<ResolvedValue> {
    if path == "z"
        && let GeoType::Point { z: Some(value), .. } = object.geo
    {
        return Some(ResolvedValue::Number(value));
    }

    if let Some(field) = object.fields.get(path) {
        return Some(match field {
            FieldValue::Number(value) => ResolvedValue::Number(*value),
            FieldValue::Text(value) => ResolvedValue::Text(value.clone()),
            FieldValue::Json(raw) => serde_json::from_str::<serde_json::Value>(raw)
                .map(ResolvedValue::Json)
                .unwrap_or_else(|_| ResolvedValue::Text(raw.clone())),
        });
    }

    if let Some((root, nested)) = path.split_once('.')
        && let Some(value) = object.fields.get_json_path(root, nested)
    {
        return Some(json_value_to_resolved(value));
    }

    object
        .geo
        .json_value()
        .and_then(|value| get_json_path(value, path))
        .cloned()
        .map(json_value_to_resolved)
}

fn json_value_to_resolved(value: serde_json::Value) -> ResolvedValue {
    match value {
        serde_json::Value::Number(number) => ResolvedValue::Number(number.as_f64().unwrap_or(0.0)),
        serde_json::Value::String(value) => ResolvedValue::Text(value),
        other => ResolvedValue::Json(other),
    }
}

fn numeric_value(value: &ResolvedValue) -> Option<f64> {
    match value {
        ResolvedValue::Number(number) => Some(*number),
        ResolvedValue::Text(text) => text.parse::<f64>().ok(),
        ResolvedValue::Json(value) => value.as_f64(),
    }
}

fn string_value(value: &ResolvedValue) -> String {
    match value {
        ResolvedValue::Number(number) => number.to_string(),
        ResolvedValue::Text(text) => text.clone(),
        ResolvedValue::Json(json) => json.to_string(),
    }
}

fn format_object(
    object: &Object,
    format: OutputFormat,
    area: Option<&Area>,
    clip: bool,
) -> IndexResult<Option<GeoType>> {
    let value = match format {
        OutputFormat::Objects => {
            if clip {
                Some(clip_geo(object.geo.clone(), area)?)
            } else {
                Some(object.geo.clone())
            }
        }
        OutputFormat::Points => Some(pointified(&object.geo)?),
        OutputFormat::Bounds => object.geo.envelope()?.map(GeoType::Bounds),
        OutputFormat::Hashes { precision } => {
            let (lat, lon) = point_from_geo(&object.geo)?;
            Some(GeoType::Hash(encode_geohash(lat, lon, precision as usize)))
        }
        OutputFormat::Ids | OutputFormat::Count => None,
    };
    Ok(value)
}

fn pointified(geo: &GeoType) -> IndexResult<GeoType> {
    let (lat, lon) = point_from_geo(geo)?;
    Ok(GeoType::Point { lat, lon, z: None })
}

fn point_from_geo(geo: &GeoType) -> IndexResult<(f64, f64)> {
    if let Some((lat, lon)) = geo.point_coordinates() {
        return Ok((lat, lon));
    }
    let envelope = geo.envelope()?.ok_or_else(|| {
        IndexError::Geo(GeoError::InvalidGeometry("non-spatial object".to_owned()))
    })?;
    Ok(envelope.center())
}

fn clip_geo(geo: GeoType, area: Option<&Area>) -> IndexResult<GeoType> {
    let Some(area) = area else {
        return Ok(geo);
    };

    match &geo {
        GeoType::String(_) => return Err(IndexError::UnsupportedClip),
        GeoType::Point { .. } => {
            return area
                .intersects_geo(&geo)
                .map_err(Into::into)
                .and_then(|inside| {
                    if inside {
                        Ok(geo)
                    } else {
                        Err(IndexError::UnsupportedClip)
                    }
                });
        }
        _ => {}
    }

    let geometry = geo.to_geometry()?.ok_or(IndexError::UnsupportedClip)?;
    let clipped = clip_geometry_to_area(&geometry, area).map_err(map_clip_error)?;
    Ok(GeoType::GeoJson(geometry_to_geojson_value(&clipped)))
}

fn map_clip_error(error: GeoError) -> IndexError {
    match error {
        GeoError::InvalidGeometry(_) => IndexError::UnsupportedClip,
        other => IndexError::Geo(other),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use latlng_geo::{Area, BoundingBox, FieldMap, FieldValue, GeoType, Object};

    #[cfg(feature = "parallel")]
    use super::{
        AreaPredicate, WhereInFilter, area_candidates_owned, nearby_candidates_owned,
        string_snapshot_candidates_owned,
    };
    use super::{
        OutputFormat, SearchCandidate, SearchOptions, SortOrder, SpatialIndex, WhereComparison,
        WhereExprFilter, WhereFilter, apply_search_options, nearby_candidates,
        snapshot_candidates_owned,
    };

    fn sample_object(id: &str, lat: f64, lon: f64, speed: f64) -> Object {
        let mut fields = FieldMap::new();
        fields.insert("speed", FieldValue::Number(speed));
        Object {
            id: id.to_owned(),
            geo: GeoType::Point { lat, lon, z: None },
            fields,
            expires_at: None,
        }
    }

    #[test]
    fn spatial_index_nearby_orders_by_distance() {
        let mut index = SpatialIndex::new();
        index.insert("a", &GeoType::point(52.52, 13.405)).unwrap();
        index.insert("b", &GeoType::point(52.53, 13.405)).unwrap();
        let ids = index.nearby_ids(52.52, 13.405, 20_000.0);
        assert_eq!(ids.first().unwrap(), "a");
    }

    #[test]
    fn options_apply_where_and_expr_filters() {
        let objects = [
            sample_object("alpha", 52.52, 13.405, 80.0),
            sample_object("bravo", 52.53, 13.405, 10.0),
        ];
        let candidates = nearby_candidates(objects.iter(), 52.52, 13.405);
        let options = SearchOptions {
            where_filters: vec![WhereFilter {
                field: "speed".to_owned(),
                comparison: WhereComparison::Range {
                    min: 50.0,
                    max: 100.0,
                },
            }],
            where_expr_filters: vec![WhereExprFilter {
                expression: "speed >= 70".to_owned(),
            }],
            output: OutputFormat::Ids,
            ..SearchOptions::default()
        };
        let results = apply_search_options(candidates, &options, None, false).unwrap();
        assert_eq!(results.count, 1);
        assert_eq!(results.results[0].id, "alpha");
    }

    #[test]
    fn descending_sort_reverses_id_order() {
        let alpha = sample_object("alpha", 1.0, 1.0, 0.0);
        let bravo = sample_object("bravo", 1.0, 1.0, 0.0);
        let candidates = vec![
            SearchCandidate {
                object: &alpha,
                distance_meters: None,
            },
            SearchCandidate {
                object: &bravo,
                distance_meters: None,
            },
        ];
        let options = SearchOptions {
            sort: SortOrder::Desc,
            output: OutputFormat::Ids,
            ..SearchOptions::default()
        };
        let results = apply_search_options(candidates, &options, None, false).unwrap();
        assert_eq!(results.results[0].id, "bravo");
    }

    #[test]
    fn clip_on_bounds_returns_actual_intersection_geometry() {
        let object = Object {
            id: "rect".to_owned(),
            geo: GeoType::Bounds(BoundingBox::new(0.0, 0.0, 2.0, 2.0)),
            fields: FieldMap::new(),
            expires_at: None,
        };
        let results = apply_search_options(
            vec![SearchCandidate {
                object: &object,
                distance_meters: None,
            }],
            &SearchOptions {
                clip: true,
                output: OutputFormat::Objects,
                ..SearchOptions::default()
            },
            Some(&Area::Bounds(BoundingBox::new(1.0, 1.0, 3.0, 3.0))),
            true,
        )
        .unwrap();

        let clipped = results.results[0].object.clone().unwrap();
        assert_eq!(
            clipped.envelope().unwrap().unwrap(),
            BoundingBox::new(1.0, 1.0, 2.0, 2.0)
        );
    }

    #[test]
    fn clip_on_geojson_polygon_returns_clipped_geometry_not_search_bounds() {
        let object = Object {
            id: "poly".to_owned(),
            geo: GeoType::GeoJson(serde_json::json!({
                "type": "Polygon",
                "coordinates": [[
                    [0.0, 0.0],
                    [2.0, 0.0],
                    [2.0, 2.0],
                    [0.0, 2.0],
                    [0.0, 0.0]
                ]]
            })),
            fields: FieldMap::new(),
            expires_at: None,
        };
        let search_bounds = BoundingBox::new(1.0, 1.0, 3.0, 3.0);
        let results = apply_search_options(
            vec![SearchCandidate {
                object: &object,
                distance_meters: None,
            }],
            &SearchOptions {
                clip: true,
                output: OutputFormat::Objects,
                ..SearchOptions::default()
            },
            Some(&Area::Bounds(search_bounds)),
            true,
        )
        .unwrap();

        let clipped = results.results[0].object.clone().unwrap();
        assert_eq!(
            clipped.envelope().unwrap().unwrap(),
            BoundingBox::new(1.0, 1.0, 2.0, 2.0)
        );
        assert_ne!(clipped.envelope().unwrap().unwrap(), search_bounds);
    }

    #[test]
    fn clip_on_point_returns_same_point() {
        let object = sample_object("point", 1.5, 1.5, 0.0);
        let results = apply_search_options(
            vec![SearchCandidate {
                object: &object,
                distance_meters: None,
            }],
            &SearchOptions {
                clip: true,
                output: OutputFormat::Objects,
                ..SearchOptions::default()
            },
            Some(&Area::Bounds(BoundingBox::new(1.0, 1.0, 3.0, 3.0))),
            true,
        )
        .unwrap();

        assert_eq!(results.results[0].object, Some(object.geo.clone()));
    }

    #[test]
    fn clip_on_hash_returns_geojson_geometry() {
        let object = Object {
            id: "hash".to_owned(),
            geo: GeoType::Hash(latlng_geo::encode_geohash(52.52, 13.405, 4)),
            fields: FieldMap::new(),
            expires_at: None,
        };
        let results = apply_search_options(
            vec![SearchCandidate {
                object: &object,
                distance_meters: None,
            }],
            &SearchOptions {
                clip: true,
                output: OutputFormat::Objects,
                ..SearchOptions::default()
            },
            Some(&Area::Bounds(BoundingBox::new(52.0, 13.0, 53.0, 14.0))),
            true,
        )
        .unwrap();

        assert!(matches!(
            results.results[0].object,
            Some(GeoType::GeoJson(_))
        ));
    }

    #[test]
    fn non_object_outputs_ignore_clip() {
        let object = Object {
            id: "poly".to_owned(),
            geo: GeoType::GeoJson(serde_json::json!({
                "type": "Polygon",
                "coordinates": [[
                    [0.0, 0.0],
                    [2.0, 0.0],
                    [2.0, 2.0],
                    [0.0, 2.0],
                    [0.0, 0.0]
                ]]
            })),
            fields: FieldMap::new(),
            expires_at: None,
        };
        let results = apply_search_options(
            vec![SearchCandidate {
                object: &object,
                distance_meters: None,
            }],
            &SearchOptions {
                clip: true,
                output: OutputFormat::Bounds,
                ..SearchOptions::default()
            },
            Some(&Area::Bounds(BoundingBox::new(1.0, 1.0, 3.0, 3.0))),
            true,
        )
        .unwrap();

        assert_eq!(
            results.results[0].object,
            Some(GeoType::Bounds(BoundingBox::new(0.0, 0.0, 2.0, 2.0)))
        );
    }

    #[test]
    fn string_clip_is_unsupported() {
        let object = Object {
            id: "text".to_owned(),
            geo: GeoType::String("hello".to_owned()),
            fields: FieldMap::new(),
            expires_at: None,
        };
        let error = apply_search_options(
            vec![SearchCandidate {
                object: &object,
                distance_meters: None,
            }],
            &SearchOptions {
                clip: true,
                output: OutputFormat::Objects,
                ..SearchOptions::default()
            },
            Some(&Area::Bounds(BoundingBox::new(0.0, 0.0, 1.0, 1.0))),
            true,
        )
        .unwrap_err();

        assert!(matches!(error, super::IndexError::UnsupportedClip));
    }

    #[test]
    fn count_output_returns_count_without_result_payloads() {
        let objects = (0..512)
            .map(|index| sample_object(&format!("obj-{index:03}"), 52.0, 13.0, index as f64))
            .collect::<Vec<_>>();

        let results = apply_search_options(
            snapshot_candidates_owned(objects),
            &SearchOptions {
                cursor: 0,
                limit: 25,
                output: OutputFormat::Count,
                ..SearchOptions::default()
            },
            None,
            false,
        )
        .unwrap();

        assert_eq!(results.count, 512);
        assert!(results.results.is_empty());
        assert_eq!(results.cursor, 25);
    }

    #[cfg(feature = "parallel")]
    #[test]
    fn parallel_nearby_owned_matches_borrowed_results() {
        let objects = (0..320)
            .map(|index| {
                sample_object(
                    &format!("obj-{index:03}"),
                    52.0 + index as f64 * 0.0001,
                    13.0,
                    index as f64,
                )
            })
            .collect::<Vec<_>>();
        let options = SearchOptions {
            output: OutputFormat::Ids,
            ..SearchOptions::default()
        };

        let serial = apply_search_options(
            nearby_candidates(objects.iter(), 52.0, 13.0),
            &options,
            None,
            false,
        )
        .unwrap();
        let parallel = apply_search_options(
            nearby_candidates_owned(objects.clone(), 52.0, 13.0),
            &options,
            None,
            false,
        )
        .unwrap();

        assert_eq!(parallel, serial);
    }

    #[cfg(feature = "parallel")]
    #[test]
    fn parallel_within_owned_matches_borrowed_results() {
        let objects = (0..192)
            .map(|index| {
                let mut object = sample_object(
                    &format!("obj-{index:03}"),
                    52.0 + index as f64 * 0.001,
                    13.0 + index as f64 * 0.001,
                    index as f64,
                );
                object
                    .fields
                    .insert("speed", FieldValue::Number((index % 10) as f64));
                object
            })
            .collect::<Vec<_>>();
        let area = Area::Bounds(BoundingBox::new(52.02, 13.02, 52.12, 13.12));
        let options = SearchOptions {
            output: OutputFormat::Ids,
            where_filters: vec![WhereFilter {
                field: "speed".to_owned(),
                comparison: WhereComparison::Range { min: 2.0, max: 6.0 },
            }],
            ..SearchOptions::default()
        };

        let serial_candidates = objects
            .iter()
            .filter_map(|object| {
                area.contains_geo(&object.geo)
                    .ok()
                    .filter(|contains| *contains)
                    .map(|_| SearchCandidate {
                        object,
                        distance_meters: None,
                    })
            })
            .collect::<Vec<_>>();
        let serial = apply_search_options(serial_candidates, &options, Some(&area), false).unwrap();
        let parallel = apply_search_options(
            area_candidates_owned(objects.clone(), &area, AreaPredicate::Within).unwrap(),
            &options,
            Some(&area),
            false,
        )
        .unwrap();

        assert_eq!(parallel, serial);
    }

    #[cfg(feature = "parallel")]
    #[test]
    fn parallel_intersects_owned_matches_borrowed_results_with_clip() {
        let objects = (0..160)
            .map(|index| Object {
                id: format!("box-{index:03}"),
                geo: GeoType::Bounds(BoundingBox::new(
                    index as f64 * 0.01,
                    index as f64 * 0.01,
                    index as f64 * 0.01 + 1.0,
                    index as f64 * 0.01 + 1.0,
                )),
                fields: FieldMap::new(),
                expires_at: None,
            })
            .collect::<Vec<_>>();
        let area = Area::Bounds(BoundingBox::new(0.5, 0.5, 2.0, 2.0));
        let options = SearchOptions {
            clip: true,
            output: OutputFormat::Objects,
            ..SearchOptions::default()
        };

        let serial_candidates = objects
            .iter()
            .filter_map(|object| {
                area.intersects_geo(&object.geo)
                    .ok()
                    .filter(|intersects| *intersects)
                    .map(|_| SearchCandidate {
                        object,
                        distance_meters: None,
                    })
            })
            .collect::<Vec<_>>();
        let serial = apply_search_options(serial_candidates, &options, Some(&area), true).unwrap();
        let parallel = apply_search_options(
            area_candidates_owned(objects.clone(), &area, AreaPredicate::Intersects).unwrap(),
            &options,
            Some(&area),
            true,
        )
        .unwrap();

        assert_eq!(parallel, serial);
    }

    #[cfg(feature = "parallel")]
    #[test]
    fn parallel_scan_owned_matches_borrowed_results() {
        let objects = (0..192)
            .map(|index| sample_object(&format!("obj-{index:03}"), 52.0, 13.0, index as f64))
            .collect::<Vec<_>>();
        let options = SearchOptions {
            output: OutputFormat::Ids,
            where_expr_filters: vec![WhereExprFilter {
                expression: "speed >= 64 && speed <= 128".to_owned(),
            }],
            ..SearchOptions::default()
        };

        let serial = apply_search_options(
            objects
                .iter()
                .map(|object| SearchCandidate {
                    object,
                    distance_meters: None,
                })
                .collect::<Vec<_>>(),
            &options,
            None,
            false,
        )
        .unwrap();
        let parallel = apply_search_options(
            snapshot_candidates_owned(objects.clone()),
            &options,
            None,
            false,
        )
        .unwrap();

        assert_eq!(parallel, serial);
    }

    #[cfg(feature = "parallel")]
    #[test]
    fn parallel_search_owned_matches_borrowed_results() {
        let objects = (0..192)
            .map(|index| Object {
                id: format!("msg-{index:03}"),
                geo: GeoType::String(format!("note-{index}")),
                fields: {
                    let mut fields = FieldMap::new();
                    fields.insert(
                        "tag",
                        FieldValue::Text(if index % 2 == 0 { "keep" } else { "drop" }.to_owned()),
                    );
                    fields
                },
                expires_at: None,
            })
            .collect::<Vec<_>>();
        let options = SearchOptions {
            output: OutputFormat::Ids,
            match_pattern: Some("msg-*".to_owned()),
            where_in_filters: vec![WhereInFilter {
                field: "tag".to_owned(),
                values: vec!["keep".to_owned()],
            }],
            ..SearchOptions::default()
        };

        let serial = apply_search_options(
            objects
                .iter()
                .filter(|object| matches!(object.geo, GeoType::String(_)))
                .map(|object| SearchCandidate {
                    object,
                    distance_meters: None,
                })
                .collect::<Vec<_>>(),
            &options,
            None,
            false,
        )
        .unwrap();
        let parallel = apply_search_options(
            string_snapshot_candidates_owned(objects.clone()),
            &options,
            None,
            false,
        )
        .unwrap();

        assert_eq!(parallel, serial);
    }

    proptest! {
        #[test]
        fn nearby_ordering_is_stable(latitudes in proptest::collection::vec(-80.0f64..80.0, 1..16)) {
            let objects = latitudes
                .iter()
                .enumerate()
                .map(|(index, lat)| sample_object(&format!("obj-{index}"), *lat, 13.0, index as f64))
                .collect::<Vec<_>>();
            let candidates = nearby_candidates(objects.iter(), 0.0, 13.0);
            let results = apply_search_options(candidates, &SearchOptions { output: OutputFormat::Ids, ..SearchOptions::default() }, None, false).unwrap();
            let mut expected = objects
                .iter()
                .map(|object| {
                    let distance = super::candidate_distance(object, 0.0, 13.0).unwrap();
                    (distance, object.id.clone())
                })
                .collect::<Vec<_>>();
            expected.sort_by(|left, right| left.0.partial_cmp(&right.0).unwrap().then_with(|| left.1.cmp(&right.1)));
            let actual = results.results.iter().map(|item| item.id.clone()).collect::<Vec<_>>();
            prop_assert_eq!(actual, expected.into_iter().map(|(_, id)| id).collect::<Vec<_>>());
        }

        #[test]
        fn cursor_pagination_is_stable(speeds in proptest::collection::vec(0.0f64..120.0, 5..24)) {
            let objects = speeds
                .iter()
                .enumerate()
                .map(|(index, speed)| sample_object(&format!("obj-{index:02}"), 52.0 + index as f64 * 0.001, 13.0, *speed))
                .collect::<Vec<_>>();
            let all_candidates = nearby_candidates(objects.iter(), 52.0, 13.0);
            let full = apply_search_options(
                all_candidates.clone(),
                &SearchOptions {
                    limit: 100,
                    output: OutputFormat::Ids,
                    ..SearchOptions::default()
                },
                None,
                false,
            ).unwrap();
            let first = apply_search_options(
                all_candidates.clone(),
                &SearchOptions {
                    limit: 5,
                    output: OutputFormat::Ids,
                    ..SearchOptions::default()
                },
                None,
                false,
            ).unwrap();
            let paged = if first.cursor == 0 {
                first.results.iter().map(|item| item.id.clone()).collect::<Vec<_>>()
            } else {
                let second = apply_search_options(
                    all_candidates,
                    &SearchOptions {
                        cursor: first.cursor,
                        limit: 100,
                        output: OutputFormat::Ids,
                        ..SearchOptions::default()
                    },
                    None,
                    false,
                ).unwrap();
                let mut paged = first.results.iter().map(|item| item.id.clone()).collect::<Vec<_>>();
                paged.extend(second.results.iter().map(|item| item.id.clone()));
                paged
            };

            prop_assert_eq!(paged, full.results.iter().map(|item| item.id.clone()).collect::<Vec<_>>());
        }

        #[test]
        fn within_candidates_cover_points_inside_bounds(
            points in proptest::collection::vec((-80.0f64..80.0, -170.0f64..170.0), 1..20),
            min_lat in -70.0f64..0.0,
            min_lon in -160.0f64..0.0,
            height in 0.1f64..20.0,
            width in 0.1f64..20.0,
        ) {
            let bounds = latlng_geo::BoundingBox::new(min_lat, min_lon, min_lat + height, min_lon + width);
            let mut index = SpatialIndex::new();
            for (idx, (lat, lon)) in points.iter().copied().enumerate() {
                index.insert(format!("p-{idx}"), &GeoType::point(lat, lon)).unwrap();
            }
            let ids = index.within_candidate_ids(bounds);
            for (idx, (lat, lon)) in points.iter().copied().enumerate() {
                let expected = bounds.contains_point(lat, lon);
                prop_assert_eq!(ids.contains(&format!("p-{idx}")), expected);
            }
        }

        #[test]
        fn intersecting_candidates_cover_overlapping_bounds(
            a_min_lat in -70.0f64..70.0,
            a_min_lon in -160.0f64..160.0,
            a_height in 0.1f64..10.0,
            a_width in 0.1f64..10.0,
            b_shift_lat in -5.0f64..5.0,
            b_shift_lon in -5.0f64..5.0,
            b_height in 0.1f64..10.0,
            b_width in 0.1f64..10.0,
        ) {
            let a = latlng_geo::BoundingBox::new(a_min_lat, a_min_lon, a_min_lat + a_height, a_min_lon + a_width);
            let b = latlng_geo::BoundingBox::new(
                a_min_lat + b_shift_lat,
                a_min_lon + b_shift_lon,
                a_min_lat + b_shift_lat + b_height,
                a_min_lon + b_shift_lon + b_width,
            );
            let mut index = SpatialIndex::new();
            index.insert("a", &GeoType::Bounds(a)).unwrap();
            let ids = index.intersecting_candidate_ids(b);
            prop_assert_eq!(ids.contains(&"a".to_owned()), a.intersects(b));
        }
    }
}
