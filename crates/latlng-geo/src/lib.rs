#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::f64::consts::PI;

use geo::algorithm::bool_ops::BooleanOps;
use geo::algorithm::bounding_rect::BoundingRect;
use geo::algorithm::contains::Contains;
use geo::algorithm::intersects::Intersects;
use geo::{Coord, LineString, MultiLineString, MultiPoint, MultiPolygon, Polygon};
use geo_types::{Geometry, GeometryCollection, Point, Rect};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

const EARTH_RADIUS_METERS: f64 = 6_371_008.8;
const GEOHASH_ALPHABET: &[u8; 32] = b"0123456789bcdefghjkmnpqrstuvwxyz";

#[derive(Debug, Error)]
pub enum GeoError {
    #[error("unsupported geojson structure")]
    UnsupportedGeoJson,
    #[error("invalid geohash")]
    InvalidGeohash,
    #[error("invalid quadkey")]
    InvalidQuadkey,
    #[error("invalid json path")]
    InvalidJsonPath,
    #[error("reference area must be resolved by the caller")]
    UnresolvedReference,
    #[error("invalid geometry: {0}")]
    InvalidGeometry(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoundingBox {
    pub min_lat: f64,
    pub min_lon: f64,
    pub max_lat: f64,
    pub max_lon: f64,
}

impl BoundingBox {
    pub fn new(min_lat: f64, min_lon: f64, max_lat: f64, max_lon: f64) -> Self {
        Self {
            min_lat: min_lat.min(max_lat),
            min_lon: min_lon.min(max_lon),
            max_lat: max_lat.max(min_lat),
            max_lon: max_lon.max(min_lon),
        }
    }

    pub fn from_point(lat: f64, lon: f64) -> Self {
        Self::new(lat, lon, lat, lon)
    }

    pub fn center(self) -> (f64, f64) {
        (
            (self.min_lat + self.max_lat) / 2.0,
            (self.min_lon + self.max_lon) / 2.0,
        )
    }

    pub fn contains_point(self, lat: f64, lon: f64) -> bool {
        (self.min_lat..=self.max_lat).contains(&lat) && (self.min_lon..=self.max_lon).contains(&lon)
    }

    pub fn contains(self, other: Self) -> bool {
        self.min_lat <= other.min_lat
            && self.min_lon <= other.min_lon
            && self.max_lat >= other.max_lat
            && self.max_lon >= other.max_lon
    }

    pub fn intersects(self, other: Self) -> bool {
        self.min_lat <= other.max_lat
            && self.max_lat >= other.min_lat
            && self.min_lon <= other.max_lon
            && self.max_lon >= other.min_lon
    }

    pub fn union(self, other: Self) -> Self {
        Self::new(
            self.min_lat.min(other.min_lat),
            self.min_lon.min(other.min_lon),
            self.max_lat.max(other.max_lat),
            self.max_lon.max(other.max_lon),
        )
    }

    pub fn to_rect(self) -> Rect<f64> {
        Rect::new(
            Coord {
                x: self.min_lon,
                y: self.min_lat,
            },
            Coord {
                x: self.max_lon,
                y: self.max_lat,
            },
        )
    }

    pub fn to_geojson_value(self) -> Value {
        serde_json::json!({
            "type": "Polygon",
            "coordinates": [[
                [self.min_lon, self.min_lat],
                [self.max_lon, self.min_lat],
                [self.max_lon, self.max_lat],
                [self.min_lon, self.max_lat],
                [self.min_lon, self.min_lat]
            ]]
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum FieldValue {
    Number(f64),
    Text(String),
    Json(String),
}

impl FieldValue {
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(value) => Some(*value),
            Self::Text(value) => value.parse::<f64>().ok(),
            Self::Json(value) => serde_json::from_str::<Value>(value)
                .ok()
                .and_then(|json| json.as_f64()),
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Number(_) => None,
            Self::Text(value) | Self::Json(value) => Some(value.as_str()),
        }
    }

    pub fn matches_regex(&self, regex: &Regex) -> bool {
        self.as_text().is_some_and(|value| regex.is_match(value))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FieldMap {
    values: BTreeMap<String, FieldValue>,
}

impl FieldMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, key: impl Into<String>, value: FieldValue) -> Option<FieldValue> {
        self.values.insert(key.into(), value)
    }

    pub fn remove(&mut self, key: &str) -> Option<FieldValue> {
        self.values.remove(key)
    }

    pub fn get(&self, key: &str) -> Option<&FieldValue> {
        self.values.get(key)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &FieldValue)> {
        self.values.iter().map(|(key, value)| (key.as_str(), value))
    }

    pub fn get_number_or_zero(&self, key: &str) -> f64 {
        self.get(key).and_then(FieldValue::as_number).unwrap_or(0.0)
    }

    pub fn get_json_path(&self, key: &str, path: &str) -> Option<Value> {
        let value = match self.get(key)? {
            FieldValue::Json(raw) => serde_json::from_str::<Value>(raw).ok()?,
            FieldValue::Text(text) => Value::String(text.clone()),
            FieldValue::Number(number) => serde_json::json!(number),
        };
        get_json_path(&value, path).cloned()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GeoType {
    Point { lat: f64, lon: f64, z: Option<f64> },
    Bounds(BoundingBox),
    Hash(String),
    GeoJson(Value),
    String(String),
}

impl GeoType {
    pub fn point(lat: f64, lon: f64) -> Self {
        Self::Point { lat, lon, z: None }
    }

    pub fn is_spatial(&self) -> bool {
        !matches!(self, Self::String(_))
    }

    pub fn point_coordinates(&self) -> Option<(f64, f64)> {
        match self {
            Self::Point { lat, lon, .. } => Some((*lat, *lon)),
            _ => None,
        }
    }

    pub fn envelope(&self) -> Result<Option<BoundingBox>, GeoError> {
        match self {
            Self::Point { lat, lon, .. } => Ok(Some(BoundingBox::from_point(*lat, *lon))),
            Self::Bounds(bounds) => Ok(Some(*bounds)),
            Self::Hash(value) => Ok(Some(decode_geohash_bbox(value)?)),
            Self::GeoJson(value) => Ok(Some(bounding_box_from_geojson(value)?)),
            Self::String(_) => Ok(None),
        }
    }

    pub fn to_geometry(&self) -> Result<Option<Geometry<f64>>, GeoError> {
        match self {
            Self::Point { lat, lon, .. } => Ok(Some(Geometry::Point(Point::new(*lon, *lat)))),
            Self::Bounds(bounds) => Ok(Some(Geometry::Rect(bounds.to_rect()))),
            Self::Hash(value) => Ok(Some(Geometry::Rect(decode_geohash_bbox(value)?.to_rect()))),
            Self::GeoJson(value) => Ok(Some(geojson_value_to_geometry(value)?)),
            Self::String(_) => Ok(None),
        }
    }

    pub fn to_geojson_value(&self) -> Result<Value, GeoError> {
        match self {
            Self::Point { lat, lon, z } => {
                let mut coords = vec![serde_json::json!(lon), serde_json::json!(lat)];
                if let Some(value) = z {
                    coords.push(serde_json::json!(value));
                }
                Ok(serde_json::json!({ "type": "Point", "coordinates": coords }))
            }
            Self::Bounds(bounds) => Ok(bounds.to_geojson_value()),
            Self::Hash(value) => Ok(decode_geohash_bbox(value)?.to_geojson_value()),
            Self::GeoJson(value) => Ok(value.clone()),
            Self::String(text) => Ok(Value::String(text.clone())),
        }
    }

    pub fn json_value(&self) -> Option<&Value> {
        match self {
            Self::GeoJson(value) => Some(value),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Object {
    pub id: String,
    pub geo: GeoType,
    pub fields: FieldMap,
    pub expires_at: Option<u64>,
}

impl Object {
    pub fn envelope(&self) -> Result<Option<BoundingBox>, GeoError> {
        self.geo.envelope()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Area {
    Circle {
        lat: f64,
        lon: f64,
        meters: f64,
    },
    Bounds(BoundingBox),
    Hash(String),
    GeoJson(Value),
    Tile {
        x: u32,
        y: u32,
        z: u32,
    },
    Quadkey(String),
    Sector {
        lat: f64,
        lon: f64,
        meters: f64,
        bearing1: f64,
        bearing2: f64,
    },
    Reference {
        collection: String,
        id: String,
    },
}

impl Area {
    pub fn envelope(&self) -> Result<BoundingBox, GeoError> {
        match self {
            Self::Circle { lat, lon, meters } => circle_envelope(*lat, *lon, *meters),
            Self::Bounds(bounds) => Ok(*bounds),
            Self::Hash(value) => decode_geohash_bbox(value),
            Self::GeoJson(value) => bounding_box_from_geojson(value),
            Self::Tile { x, y, z } => tile_bounds(*x, *y, *z),
            Self::Quadkey(value) => tile_bounds_from_quadkey(value),
            Self::Sector {
                lat, lon, meters, ..
            } => circle_envelope(*lat, *lon, *meters),
            Self::Reference { .. } => Err(GeoError::UnresolvedReference),
        }
    }

    pub fn to_geometry(&self) -> Result<Geometry<f64>, GeoError> {
        match self {
            Self::Circle { lat, lon, meters } => {
                Ok(Geometry::Polygon(circle_polygon(*lat, *lon, *meters, 64)))
            }
            Self::Bounds(bounds) => Ok(Geometry::Rect(bounds.to_rect())),
            Self::Hash(value) => Ok(Geometry::Rect(decode_geohash_bbox(value)?.to_rect())),
            Self::GeoJson(value) => geojson_value_to_geometry(value),
            Self::Tile { x, y, z } => Ok(Geometry::Rect(tile_bounds(*x, *y, *z)?.to_rect())),
            Self::Quadkey(value) => Ok(Geometry::Rect(tile_bounds_from_quadkey(value)?.to_rect())),
            Self::Sector {
                lat,
                lon,
                meters,
                bearing1,
                bearing2,
            } => Ok(Geometry::Polygon(sector_polygon(
                *lat, *lon, *meters, *bearing1, *bearing2, 64,
            ))),
            Self::Reference { .. } => Err(GeoError::UnresolvedReference),
        }
    }

    pub fn contains_geo(&self, geo: &GeoType) -> Result<bool, GeoError> {
        let area_geometry = self.to_geometry()?;
        let Some(target) = geo.to_geometry()? else {
            return Ok(false);
        };
        Ok(area_contains_geometry(&area_geometry, &target))
    }

    pub fn intersects_geo(&self, geo: &GeoType) -> Result<bool, GeoError> {
        let area_geometry = self.to_geometry()?;
        let Some(target) = geo.to_geometry()? else {
            return Ok(false);
        };
        Ok(area_geometry.intersects(&target))
    }
}

pub fn geometry_to_geojson_value(geometry: &Geometry<f64>) -> Value {
    serde_json::to_value(geojson::Geometry::new(geojson::Value::from(geometry)))
        .unwrap_or(Value::Null)
}

pub fn clip_geometry_to_area(
    geometry: &Geometry<f64>,
    area: &Area,
) -> Result<Geometry<f64>, GeoError> {
    let area_surface = area_surface(area)?;
    clip_geometry_with_surface(geometry, &area_surface, area)
}

fn clip_geometry_with_surface(
    geometry: &Geometry<f64>,
    area_surface: &MultiPolygon<f64>,
    area: &Area,
) -> Result<Geometry<f64>, GeoError> {
    match geometry {
        Geometry::Point(point) => {
            let point_geo = GeoType::Point {
                lat: point.y(),
                lon: point.x(),
                z: None,
            };
            if area.intersects_geo(&point_geo)? {
                Ok(Geometry::Point(*point))
            } else {
                Err(GeoError::InvalidGeometry(
                    "geometry does not intersect clip area".to_owned(),
                ))
            }
        }
        Geometry::MultiPoint(points) => {
            let kept = points
                .iter()
                .filter(|point| {
                    area.intersects_geo(&GeoType::Point {
                        lat: point.y(),
                        lon: point.x(),
                        z: None,
                    })
                    .unwrap_or(false)
                })
                .copied()
                .collect::<Vec<_>>();
            geometry_from_points(kept)
        }
        Geometry::Line(line) => clip_multiline(
            area_surface,
            &MultiLineString::new(vec![LineString::from(vec![line.start, line.end])]),
        ),
        Geometry::LineString(line_string) => clip_multiline(
            area_surface,
            &MultiLineString::new(vec![line_string.clone()]),
        ),
        Geometry::MultiLineString(lines) => clip_multiline(area_surface, lines),
        Geometry::Rect(rect) => clip_polygonish(area_surface, &rect.to_polygon()),
        Geometry::Triangle(triangle) => clip_polygonish(area_surface, &triangle.to_polygon()),
        Geometry::Polygon(polygon) => clip_polygonish(area_surface, polygon),
        Geometry::MultiPolygon(polygons) => {
            let clipped = area_surface.intersection(polygons);
            geometry_from_multipolygon(clipped)
        }
        Geometry::GeometryCollection(collection) => {
            let mut clipped = Vec::new();
            for item in &collection.0 {
                match clip_geometry_with_surface(item, area_surface, area) {
                    Ok(Geometry::GeometryCollection(inner)) => clipped.extend(inner.0),
                    Ok(geometry) => clipped.push(geometry),
                    Err(GeoError::InvalidGeometry(_)) => {}
                    Err(error) => return Err(error),
                }
            }
            geometry_from_collection(clipped)
        }
    }
}

fn area_surface(area: &Area) -> Result<MultiPolygon<f64>, GeoError> {
    match area.to_geometry()? {
        Geometry::Rect(rect) => Ok(MultiPolygon::new(vec![rect.to_polygon()])),
        Geometry::Polygon(polygon) => Ok(MultiPolygon::new(vec![polygon])),
        Geometry::MultiPolygon(polygons) => Ok(polygons),
        _ => Err(GeoError::InvalidGeometry(
            "clip area must be a polygonal geometry".to_owned(),
        )),
    }
}

fn clip_polygonish(
    area_surface: &MultiPolygon<f64>,
    polygon: &Polygon<f64>,
) -> Result<Geometry<f64>, GeoError> {
    let clipped = area_surface.intersection(polygon);
    geometry_from_multipolygon(clipped)
}

fn clip_multiline(
    area_surface: &MultiPolygon<f64>,
    lines: &MultiLineString<f64>,
) -> Result<Geometry<f64>, GeoError> {
    let clipped = area_surface.clip(lines, false);
    geometry_from_multiline(clipped)
}

fn geometry_from_points(points: Vec<Point<f64>>) -> Result<Geometry<f64>, GeoError> {
    match points.len() {
        0 => Err(GeoError::InvalidGeometry(
            "geometry does not intersect clip area".to_owned(),
        )),
        1 => Ok(Geometry::Point(points[0])),
        _ => Ok(Geometry::MultiPoint(MultiPoint::new(points))),
    }
}

fn geometry_from_multiline(lines: MultiLineString<f64>) -> Result<Geometry<f64>, GeoError> {
    match lines.0.len() {
        0 => Err(GeoError::InvalidGeometry(
            "geometry does not intersect clip area".to_owned(),
        )),
        1 => Ok(Geometry::LineString(lines.0[0].clone())),
        _ => Ok(Geometry::MultiLineString(lines)),
    }
}

fn geometry_from_multipolygon(polygons: MultiPolygon<f64>) -> Result<Geometry<f64>, GeoError> {
    match polygons.0.len() {
        0 => Err(GeoError::InvalidGeometry(
            "geometry does not intersect clip area".to_owned(),
        )),
        1 => Ok(Geometry::Polygon(polygons.0[0].clone())),
        _ => Ok(Geometry::MultiPolygon(polygons)),
    }
}

fn geometry_from_collection(items: Vec<Geometry<f64>>) -> Result<Geometry<f64>, GeoError> {
    match items.len() {
        0 => Err(GeoError::InvalidGeometry(
            "geometry does not intersect clip area".to_owned(),
        )),
        1 => Ok(items.into_iter().next().unwrap()),
        _ => Ok(Geometry::GeometryCollection(GeometryCollection(items))),
    }
}

pub fn haversine_distance_meters(a_lat: f64, a_lon: f64, b_lat: f64, b_lon: f64) -> f64 {
    let lat1 = a_lat.to_radians();
    let lat2 = b_lat.to_radians();
    let delta_lat = (b_lat - a_lat).to_radians();
    let delta_lon = (b_lon - a_lon).to_radians();

    let sin_lat = (delta_lat / 2.0).sin();
    let sin_lon = (delta_lon / 2.0).sin();
    let h = sin_lat * sin_lat + lat1.cos() * lat2.cos() * sin_lon * sin_lon;
    2.0 * EARTH_RADIUS_METERS * h.sqrt().atan2((1.0 - h).sqrt())
}

pub fn encode_geohash(lat: f64, lon: f64, precision: usize) -> String {
    let mut hash = String::with_capacity(precision);
    let (mut lat_min, mut lat_max) = (-90.0, 90.0);
    let (mut lon_min, mut lon_max) = (-180.0, 180.0);
    let mut ch = 0usize;
    let mut bits = 0usize;
    let mut even = true;

    while hash.len() < precision {
        if even {
            let mid = (lon_min + lon_max) / 2.0;
            if lon >= mid {
                ch = (ch << 1) | 1;
                lon_min = mid;
            } else {
                ch <<= 1;
                lon_max = mid;
            }
        } else {
            let mid = (lat_min + lat_max) / 2.0;
            if lat >= mid {
                ch = (ch << 1) | 1;
                lat_min = mid;
            } else {
                ch <<= 1;
                lat_max = mid;
            }
        }
        even = !even;
        bits += 1;
        if bits == 5 {
            hash.push(GEOHASH_ALPHABET[ch] as char);
            bits = 0;
            ch = 0;
        }
    }

    hash
}

pub fn decode_geohash_bbox(value: &str) -> Result<BoundingBox, GeoError> {
    let (mut lat_min, mut lat_max) = (-90.0, 90.0);
    let (mut lon_min, mut lon_max) = (-180.0, 180.0);
    let mut even = true;

    for byte in value.bytes() {
        let index = GEOHASH_ALPHABET
            .iter()
            .position(|candidate| *candidate == byte)
            .ok_or(GeoError::InvalidGeohash)?;

        for shift in (0..5).rev() {
            let bit = ((index >> shift) & 1) == 1;
            if even {
                let mid = (lon_min + lon_max) / 2.0;
                if bit {
                    lon_min = mid;
                } else {
                    lon_max = mid;
                }
            } else {
                let mid = (lat_min + lat_max) / 2.0;
                if bit {
                    lat_min = mid;
                } else {
                    lat_max = mid;
                }
            }
            even = !even;
        }
    }

    Ok(BoundingBox::new(lat_min, lon_min, lat_max, lon_max))
}

pub fn get_json_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    if path.is_empty() || path == "." {
        return Some(value);
    }

    path.trim_start_matches('$')
        .trim_start_matches('.')
        .split('.')
        .filter(|segment| !segment.is_empty())
        .try_fold(value, |cursor, segment| match cursor {
            Value::Object(map) => map.get(segment),
            Value::Array(items) => segment
                .parse::<usize>()
                .ok()
                .and_then(|index| items.get(index)),
            _ => None,
        })
}

pub fn set_json_path(target: &mut Value, path: &str, new_value: Value) -> Result<(), GeoError> {
    let mut segments = path
        .trim_start_matches('$')
        .trim_start_matches('.')
        .split('.')
        .filter(|segment| !segment.is_empty())
        .peekable();

    if segments.peek().is_none() {
        *target = new_value;
        return Ok(());
    }

    let mut cursor = target;
    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            match cursor {
                Value::Object(map) => {
                    map.insert(segment.to_owned(), new_value);
                    return Ok(());
                }
                Value::Array(items) => {
                    let index = segment
                        .parse::<usize>()
                        .map_err(|_| GeoError::InvalidJsonPath)?;
                    if index >= items.len() {
                        return Err(GeoError::InvalidJsonPath);
                    }
                    items[index] = new_value;
                    return Ok(());
                }
                _ => return Err(GeoError::InvalidJsonPath),
            }
        }

        cursor = match cursor {
            Value::Object(map) => map
                .entry(segment.to_owned())
                .or_insert_with(|| Value::Object(serde_json::Map::new())),
            Value::Array(items) => {
                let index = segment
                    .parse::<usize>()
                    .map_err(|_| GeoError::InvalidJsonPath)?;
                items.get_mut(index).ok_or(GeoError::InvalidJsonPath)?
            }
            _ => return Err(GeoError::InvalidJsonPath),
        };
    }

    Ok(())
}

pub fn delete_json_path(target: &mut Value, path: &str) -> Result<bool, GeoError> {
    let mut segments = path
        .trim_start_matches('$')
        .trim_start_matches('.')
        .split('.')
        .filter(|segment| !segment.is_empty())
        .peekable();

    let mut cursor = target;
    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            return match cursor {
                Value::Object(map) => Ok(map.remove(segment).is_some()),
                Value::Array(items) => {
                    let index = segment
                        .parse::<usize>()
                        .map_err(|_| GeoError::InvalidJsonPath)?;
                    if index >= items.len() {
                        return Ok(false);
                    }
                    items.remove(index);
                    Ok(true)
                }
                _ => Err(GeoError::InvalidJsonPath),
            };
        }

        cursor = match cursor {
            Value::Object(map) => map.get_mut(segment).ok_or(GeoError::InvalidJsonPath)?,
            Value::Array(items) => {
                let index = segment
                    .parse::<usize>()
                    .map_err(|_| GeoError::InvalidJsonPath)?;
                items.get_mut(index).ok_or(GeoError::InvalidJsonPath)?
            }
            _ => return Err(GeoError::InvalidJsonPath),
        };
    }

    Ok(false)
}

fn area_contains_geometry(area: &Geometry<f64>, target: &Geometry<f64>) -> bool {
    match area {
        Geometry::Rect(rect) => match target {
            Geometry::Point(point) => rect.contains(point),
            Geometry::Rect(other) => rect.to_polygon().contains(&other.to_polygon()),
            _ => rect.to_polygon().contains(target),
        },
        Geometry::Polygon(polygon) => polygon.contains(target),
        Geometry::MultiPolygon(polygons) => match target {
            Geometry::Point(point) => polygons.contains(point),
            Geometry::Line(line) => polygons.contains(line),
            Geometry::LineString(line_string) => polygons.contains(line_string),
            Geometry::Polygon(polygon) => polygons.contains(polygon),
            Geometry::Rect(rect) => polygons.contains(rect),
            _ => polygons.intersects(target),
        },
        Geometry::Point(point) => matches!(target, Geometry::Point(other) if point == other),
        _ => area.contains(target),
    }
}

fn circle_envelope(lat: f64, lon: f64, meters: f64) -> Result<BoundingBox, GeoError> {
    let lat_delta = (meters / EARTH_RADIUS_METERS) * (180.0 / PI);
    let lon_scale = lat.to_radians().cos().abs().max(1e-9);
    let lon_delta = (meters / EARTH_RADIUS_METERS) * (180.0 / PI) / lon_scale;
    Ok(BoundingBox::new(
        lat - lat_delta,
        lon - lon_delta,
        lat + lat_delta,
        lon + lon_delta,
    ))
}

fn tile_bounds(x: u32, y: u32, z: u32) -> Result<BoundingBox, GeoError> {
    let n = 2_f64.powi(i32::try_from(z).map_err(|_| GeoError::InvalidQuadkey)?);
    let min_lon = x as f64 / n * 360.0 - 180.0;
    let max_lon = (x as f64 + 1.0) / n * 360.0 - 180.0;
    let max_lat = tile_lat(y as f64, n);
    let min_lat = tile_lat(y as f64 + 1.0, n);
    Ok(BoundingBox::new(min_lat, min_lon, max_lat, max_lon))
}

fn tile_lat(y: f64, n: f64) -> f64 {
    let radians = (PI * (1.0 - 2.0 * y / n)).sinh().atan();
    radians.to_degrees()
}

fn tile_bounds_from_quadkey(value: &str) -> Result<BoundingBox, GeoError> {
    let mut x = 0_u32;
    let mut y = 0_u32;
    let z = value.len() as u32;
    for (position, digit) in value.chars().enumerate() {
        let bit = 1_u32 << (z - position as u32 - 1);
        match digit {
            '0' => {}
            '1' => x |= bit,
            '2' => y |= bit,
            '3' => {
                x |= bit;
                y |= bit;
            }
            _ => return Err(GeoError::InvalidQuadkey),
        }
    }
    tile_bounds(x, y, z)
}

fn circle_polygon(lat: f64, lon: f64, meters: f64, segments: usize) -> Polygon<f64> {
    let coords = (0..=segments)
        .map(|index| {
            let bearing = 360.0 * (index as f64) / (segments as f64);
            destination_point(lat, lon, meters, bearing)
        })
        .map(|(point_lat, point_lon)| Coord {
            x: point_lon,
            y: point_lat,
        })
        .collect::<Vec<_>>();
    Polygon::new(LineString::from(coords), vec![])
}

fn sector_polygon(
    lat: f64,
    lon: f64,
    meters: f64,
    bearing1: f64,
    bearing2: f64,
    segments: usize,
) -> Polygon<f64> {
    let mut coords = Vec::with_capacity(segments + 3);
    coords.push(Coord { x: lon, y: lat });

    let start = normalize_bearing(bearing1);
    let mut end = normalize_bearing(bearing2);
    if end <= start {
        end += 360.0;
    }

    for index in 0..=segments {
        let bearing = start + ((end - start) * (index as f64) / (segments as f64));
        let (point_lat, point_lon) = destination_point(lat, lon, meters, bearing);
        coords.push(Coord {
            x: point_lon,
            y: point_lat,
        });
    }

    coords.push(Coord { x: lon, y: lat });
    Polygon::new(LineString::from(coords), vec![])
}

fn normalize_bearing(value: f64) -> f64 {
    value.rem_euclid(360.0)
}

fn destination_point(lat: f64, lon: f64, meters: f64, bearing_degrees: f64) -> (f64, f64) {
    let angular_distance = meters / EARTH_RADIUS_METERS;
    let bearing = bearing_degrees.to_radians();
    let lat1 = lat.to_radians();
    let lon1 = lon.to_radians();

    let sin_lat1 = lat1.sin();
    let cos_lat1 = lat1.cos();
    let sin_ad = angular_distance.sin();
    let cos_ad = angular_distance.cos();

    let lat2 = (sin_lat1 * cos_ad + cos_lat1 * sin_ad * bearing.cos()).asin();
    let lon2 = lon1 + (bearing.sin() * sin_ad * cos_lat1).atan2(cos_ad - sin_lat1 * lat2.sin());

    (
        lat2.to_degrees(),
        ((lon2.to_degrees() + 540.0) % 360.0) - 180.0,
    )
}

fn bounding_box_from_geojson(value: &Value) -> Result<BoundingBox, GeoError> {
    let geometry = geojson_value_to_geometry(value)?;
    let rect = geometry
        .bounding_rect()
        .ok_or_else(|| GeoError::InvalidGeometry("missing bounding rect".to_owned()))?;
    Ok(BoundingBox::new(
        rect.min().y,
        rect.min().x,
        rect.max().y,
        rect.max().x,
    ))
}

fn geojson_value_to_geometry(value: &Value) -> Result<Geometry<f64>, GeoError> {
    let geojson = serde_json::from_value::<geojson::GeoJson>(value.clone())
        .map_err(|error| GeoError::InvalidGeometry(error.to_string()))?;
    match geojson {
        geojson::GeoJson::Geometry(geometry) => {
            let geometry: Result<Geometry<f64>, _> = geometry.try_into();
            geometry.map_err(|error| GeoError::InvalidGeometry(error.to_string()))
        }
        geojson::GeoJson::Feature(feature) => {
            let geometry = feature.geometry.ok_or(GeoError::UnsupportedGeoJson)?;
            let geometry: Result<Geometry<f64>, _> = geometry.try_into();
            geometry.map_err(|error| GeoError::InvalidGeometry(error.to_string()))
        }
        geojson::GeoJson::FeatureCollection(_) => Err(GeoError::UnsupportedGeoJson),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Area, BoundingBox, FieldMap, FieldValue, GeoType, decode_geohash_bbox, encode_geohash,
        get_json_path, haversine_distance_meters, set_json_path,
    };

    #[test]
    fn point_envelope_is_degenerate_bounds() {
        let envelope = GeoType::point(52.52, 13.405).envelope().unwrap().unwrap();
        assert_eq!(envelope, BoundingBox::new(52.52, 13.405, 52.52, 13.405));
    }

    #[test]
    fn geohash_roundtrip_stays_near_original_point() {
        let hash = encode_geohash(52.52, 13.405, 8);
        let envelope = decode_geohash_bbox(&hash).unwrap();
        let (lat, lon) = envelope.center();
        assert!(haversine_distance_meters(52.52, 13.405, lat, lon) < 50.0);
    }

    #[test]
    fn json_paths_can_be_updated() {
        let mut value = serde_json::json!({ "properties": { "speed": 10 } });
        set_json_path(&mut value, "properties.speed", serde_json::json!(20)).unwrap();
        assert_eq!(
            get_json_path(&value, "properties.speed"),
            Some(&serde_json::json!(20))
        );
    }

    #[test]
    fn area_contains_point() {
        let area = Area::Circle {
            lat: 52.52,
            lon: 13.405,
            meters: 5_000.0,
        };
        assert!(area.contains_geo(&GeoType::point(52.52, 13.41)).unwrap());
    }

    #[test]
    fn field_map_defaults_missing_numbers_to_zero() {
        let mut fields = FieldMap::new();
        fields.insert("speed", FieldValue::Number(80.0));
        assert_eq!(fields.get_number_or_zero("speed"), 80.0);
        assert_eq!(fields.get_number_or_zero("missing"), 0.0);
    }
}
