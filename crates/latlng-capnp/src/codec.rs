use latlng_core::geo::{Area, BoundingBox, FieldMap, FieldValue, GeoType, Object};
use latlng_core::geofence::{
    DetectType, GeofenceDef, GeofenceEvent, GeofenceQuery, MutationCommand,
};
use latlng_core::index::{
    OutputFormat, SearchItem, SearchOptions, SearchResults, SortOrder, WhereComparison,
};
use latlng_core::{FieldEntry, NearbyQuery, ServerInfo, SetCondition, SetRequest};
use serde::Serialize;

use crate::rpc;

pub(crate) fn fill_ok_response(mut builder: rpc::ok_response::Builder<'_>, ok: bool, error: &str) {
    builder.set_ok(ok);
    builder.set_error(error);
}

pub(crate) fn fill_bounds(mut builder: rpc::bounds::Builder<'_>, bounds: &BoundingBox) {
    builder.set_min_lat(bounds.min_lat);
    builder.set_min_lon(bounds.min_lon);
    builder.set_max_lat(bounds.max_lat);
    builder.set_max_lon(bounds.max_lon);
}

pub(crate) fn fill_server_info(mut builder: rpc::server_info::Builder<'_>, info: &ServerInfo) {
    builder.set_num_collections(info.num_collections);
    builder.set_num_objects(info.num_objects);
    builder.set_num_points(info.num_points);
    builder.set_heap_bytes(info.heap_bytes);
    builder.set_read_only(info.read_only);
    builder.set_leader(info.leader);
    builder.set_server_id(&info.server_id);
    builder.set_following(info.following.as_deref().unwrap_or(""));
    builder.set_caught_up(info.caught_up);
    builder.set_caught_up_once(info.caught_up_once);
    builder.set_last_sequence(info.last_sequence);
    builder.set_version(&info.version);
    builder.set_api_version(&info.api_version);
    builder.set_protocol_version(&info.protocol_version);
    builder.set_storage_format_version(&info.storage_format_version);
}

pub(crate) fn fill_replication_info(
    mut builder: rpc::replication_info::Builder<'_>,
    info: &ServerInfo,
) {
    builder.set_server_id(&info.server_id);
    builder.set_following(info.following.as_deref().unwrap_or(""));
    builder.set_leader(info.leader);
    builder.set_last_sequence(info.last_sequence);
}

pub(crate) fn fill_geo_object(
    mut builder: rpc::geo_object::Builder<'_>,
    geo: &GeoType,
) -> Result<(), capnp::Error> {
    match geo {
        GeoType::Point { lat, lon, z } => {
            let mut point = builder.init_point();
            point.set_lat(*lat);
            point.set_lon(*lon);
            point.set_z(z.unwrap_or(0.0));
        }
        GeoType::Bounds(bounds) => fill_bounds(builder.init_bounds(), bounds),
        GeoType::Hash(hash) => builder.set_hash(hash.as_str()),
        GeoType::GeoJson(value) => {
            let encoded = value.to_string();
            builder.set_geojson(encoded.as_str());
        }
        GeoType::String(value) => builder.set_string(value.as_str()),
    }
    Ok(())
}

pub(crate) fn fill_field_entry(mut builder: rpc::field_entry::Builder<'_>, field: &FieldEntry) {
    fill_field_value(builder.reborrow(), field.name.as_str(), &field.value);
}

fn fill_field_value(mut builder: rpc::field_entry::Builder<'_>, name: &str, value: &FieldValue) {
    builder.set_name(name);
    match value {
        FieldValue::Number(value) => builder.set_number(*value),
        FieldValue::Text(value) => builder.set_text(value.as_str()),
        FieldValue::Json(value) => builder.set_json(value.as_str()),
    }
}

pub(crate) fn field_entries_from_map(fields: &FieldMap) -> Vec<FieldEntry> {
    fields
        .iter()
        .map(|(name, value)| FieldEntry {
            name: name.to_owned(),
            value: value.clone(),
        })
        .collect()
}

pub(crate) fn fill_search_item(
    mut builder: rpc::search_result::Builder<'_>,
    item: &SearchItem,
    include_distance: bool,
) -> Result<(), capnp::Error> {
    builder.set_id(item.id.as_str());
    let object = item
        .object
        .clone()
        .unwrap_or_else(|| GeoType::String(String::new()));
    fill_geo_object(builder.reborrow().init_object(), &object)?;
    let fields = item
        .fields
        .as_ref()
        .map(field_entries_from_map)
        .unwrap_or_default();
    let mut out_fields = builder.reborrow().init_fields(fields.len() as u32);
    for (index, field) in fields.iter().enumerate() {
        fill_field_entry(out_fields.reborrow().get(index as u32), field);
    }
    builder.set_dist(if include_distance {
        item.distance_meters.unwrap_or(0.0)
    } else {
        0.0
    });
    Ok(())
}

pub(crate) fn fill_search_item_from_object(
    mut builder: rpc::search_result::Builder<'_>,
    object: &Object,
) -> Result<(), capnp::Error> {
    builder.set_id(object.id.as_str());
    fill_geo_object(builder.reborrow().init_object(), &object.geo)?;
    let mut out_fields = builder
        .reborrow()
        .init_fields(object.fields.iter().count() as u32);
    for (index, (name, value)) in object.fields.iter().enumerate() {
        fill_field_value(out_fields.reborrow().get(index as u32), name, value);
    }
    builder.set_dist(0.0);
    Ok(())
}

pub(crate) fn fill_search_response(
    mut builder: rpc::search_response::Builder<'_>,
    response: &SearchResults,
) -> Result<(), capnp::Error> {
    builder.set_ok(true);
    builder.set_cursor(response.cursor);
    builder.set_count(response.count);
    builder.set_error("");
    if search_response_is_ids_only(response) {
        let mut ids = builder.reborrow().init_ids(response.results.len() as u32);
        for (index, item) in response.results.iter().enumerate() {
            ids.set(index as u32, item.id.as_str());
        }
        let _ = builder.init_results(0);
    } else {
        let _ = builder.reborrow().init_ids(0);
        let mut items = builder
            .reborrow()
            .init_results(response.results.len() as u32);
        for (index, item) in response.results.iter().enumerate() {
            fill_search_item(items.reborrow().get(index as u32), item, true)?;
        }
    }
    Ok(())
}

pub(crate) fn fill_search_error(mut builder: rpc::search_response::Builder<'_>, error: &str) {
    builder.set_ok(false);
    builder.set_cursor(0);
    builder.set_count(0);
    builder.set_error(error);
    let _ = builder.reborrow().init_results(0);
    let _ = builder.init_ids(0);
}

fn search_response_is_ids_only(response: &SearchResults) -> bool {
    response.results.iter().all(|item| {
        item.object.is_none() && item.fields.is_none() && item.distance_meters.is_none()
    })
}

pub(crate) fn fill_geofence_event(
    mut builder: rpc::geofence_event::Builder<'_>,
    event: &GeofenceEvent,
) -> Result<(), capnp::Error> {
    builder.set_command(command_name(event.command));
    builder.set_detect(detect_to_capnp(event.detect));
    builder.set_collection(event.collection.as_str());
    builder.set_id(event.id.as_str());
    fill_geo_object(builder.reborrow().init_object(), &event.object)?;
    let fields = field_entries_from_map(&event.fields);
    let mut out_fields = builder.reborrow().init_fields(fields.len() as u32);
    for (index, field) in fields.iter().enumerate() {
        fill_field_entry(out_fields.reborrow().get(index as u32), field);
    }
    builder.set_time_ns(i64::try_from(event.timestamp_ns).unwrap_or(i64::MAX));
    builder.set_hook(event.hook.as_deref().unwrap_or(""));
    builder.set_group(event.group.as_deref().unwrap_or(""));
    if let Some(nearby) = &event.nearby {
        let mut out_nearby = builder.init_nearby();
        out_nearby.set_collection(nearby.collection.as_str());
        out_nearby.set_id(nearby.id.as_str());
        out_nearby.set_meters(nearby.meters);
    }
    Ok(())
}

pub(crate) fn set_request_from_reader(
    reader: rpc::set_request::Reader<'_>,
) -> Result<SetRequest, capnp::Error> {
    Ok(SetRequest {
        collection: read_text(reader.get_collection())?,
        id: read_text(reader.get_id())?,
        object: geo_from_reader(reader.get_object()?)?,
        fields: field_entries_from_list(reader.get_fields()?)?,
        expire_seconds: (reader.get_expire_sec() != 0).then_some(reader.get_expire_sec()),
        condition: set_condition_from_capnp(
            reader.get_condition().unwrap_or(rpc::SetCondition::Always),
        ),
    })
}

pub(crate) fn nearby_query_from_reader(
    reader: rpc::nearby_request::Reader<'_>,
) -> Result<(String, NearbyQuery), capnp::Error> {
    Ok((
        read_text(reader.get_collection())?,
        NearbyQuery {
            lat: reader.get_lat(),
            lon: reader.get_lon(),
            meters: reader.get_meters(),
            options: search_options_from_reader(reader.get_options()?)?,
        },
    ))
}

pub(crate) fn area_query_from_within(
    reader: rpc::within_request::Reader<'_>,
) -> Result<(String, Area, SearchOptions), capnp::Error> {
    Ok((
        read_text(reader.get_collection())?,
        area_from_reader(reader.get_area()?)?,
        search_options_from_reader(reader.get_options()?)?,
    ))
}

pub(crate) fn area_query_from_intersects(
    reader: rpc::intersects_request::Reader<'_>,
) -> Result<(String, Area, SearchOptions), capnp::Error> {
    Ok((
        read_text(reader.get_collection())?,
        area_from_reader(reader.get_area()?)?,
        search_options_from_reader(reader.get_options()?)?,
    ))
}

pub(crate) fn field_entries_from_list(
    reader: capnp::struct_list::Reader<'_, rpc::field_entry::Owned>,
) -> Result<Vec<FieldEntry>, capnp::Error> {
    reader.iter().map(field_entry_from_reader).collect()
}

pub(crate) fn field_entry_from_reader(
    reader: rpc::field_entry::Reader<'_>,
) -> Result<FieldEntry, capnp::Error> {
    let name = read_text(reader.get_name())?;
    let value = match reader.which().map_err(not_in_schema_error)? {
        rpc::field_entry::WhichReader::Number(value) => FieldValue::Number(value),
        rpc::field_entry::WhichReader::Text(value) => FieldValue::Text(read_text(value)?),
        rpc::field_entry::WhichReader::Json(value) => FieldValue::Json(read_text(value)?),
    };
    Ok(FieldEntry { name, value })
}

pub(crate) fn geo_from_reader(
    reader: rpc::geo_object::Reader<'_>,
) -> Result<GeoType, capnp::Error> {
    match reader.which().map_err(not_in_schema_error)? {
        rpc::geo_object::WhichReader::Point(point) => {
            let point = point?;
            Ok(GeoType::Point {
                lat: point.get_lat(),
                lon: point.get_lon(),
                z: (point.get_z() != 0.0).then_some(point.get_z()),
            })
        }
        rpc::geo_object::WhichReader::Bounds(bounds) => {
            Ok(GeoType::Bounds(bounds_from_reader(bounds?)?))
        }
        rpc::geo_object::WhichReader::Hash(hash) => Ok(GeoType::Hash(read_text(hash)?)),
        rpc::geo_object::WhichReader::Geojson(value) => Ok(GeoType::GeoJson(
            serde_json::from_str(&read_text(value)?).map_err(capnp_failed)?,
        )),
        rpc::geo_object::WhichReader::String(value) => Ok(GeoType::String(read_text(value)?)),
    }
}

pub(crate) fn bounds_from_reader(
    reader: rpc::bounds::Reader<'_>,
) -> Result<BoundingBox, capnp::Error> {
    Ok(BoundingBox::new(
        reader.get_min_lat(),
        reader.get_min_lon(),
        reader.get_max_lat(),
        reader.get_max_lon(),
    ))
}

pub(crate) fn area_from_reader(reader: rpc::area_spec::Reader<'_>) -> Result<Area, capnp::Error> {
    match reader.which().map_err(not_in_schema_error)? {
        rpc::area_spec::WhichReader::Circle(circle) => Ok(Area::Circle {
            lat: circle.get_lat(),
            lon: circle.get_lon(),
            meters: circle.get_meters(),
        }),
        rpc::area_spec::WhichReader::Bounds(bounds) => {
            Ok(Area::Bounds(bounds_from_reader(bounds?)?))
        }
        rpc::area_spec::WhichReader::Hash(hash) => Ok(Area::Hash(read_text(hash)?)),
        rpc::area_spec::WhichReader::Object(value) => Ok(Area::GeoJson(
            serde_json::from_str(&read_text(value)?).map_err(capnp_failed)?,
        )),
        rpc::area_spec::WhichReader::Tile(tile) => Ok(Area::Tile {
            x: tile.get_x(),
            y: tile.get_y(),
            z: tile.get_z(),
        }),
        rpc::area_spec::WhichReader::Quadkey(value) => Ok(Area::Quadkey(read_text(value)?)),
        rpc::area_spec::WhichReader::Sector(sector) => Ok(Area::Sector {
            lat: sector.get_lat(),
            lon: sector.get_lon(),
            meters: sector.get_meters(),
            bearing1: sector.get_bearing1(),
            bearing2: sector.get_bearing2(),
        }),
        rpc::area_spec::WhichReader::Get(reference) => Ok(Area::Reference {
            collection: read_text(reference.get_collection())?,
            id: read_text(reference.get_id())?,
        }),
    }
}

pub(crate) fn search_options_from_reader(
    reader: rpc::search_options::Reader<'_>,
) -> Result<SearchOptions, capnp::Error> {
    let where_filters = reader
        .get_where()?
        .iter()
        .map(|item| {
            Ok(latlng_core::index::WhereFilter {
                field: read_text(item.get_field())?,
                comparison: WhereComparison::Range {
                    min: item.get_min(),
                    max: item.get_max(),
                },
            })
        })
        .collect::<Result<Vec<_>, capnp::Error>>()?;
    let where_expr_filters = reader
        .get_where_expr()?
        .iter()
        .map(|item| {
            Ok(latlng_core::index::WhereExprFilter {
                expression: read_text(item.get_expression())?,
            })
        })
        .collect::<Result<Vec<_>, capnp::Error>>()?;
    let pattern = read_text(reader.get_match())?;
    Ok(SearchOptions {
        cursor: reader.get_cursor(),
        limit: if reader.get_limit() == 0 {
            SearchOptions::default().limit
        } else {
            reader.get_limit()
        },
        nofields: reader.get_nofields(),
        include_count: reader.get_include_count(),
        match_pattern: (!pattern.is_empty()).then_some(pattern),
        sort: if reader.get_asc() {
            SortOrder::Asc
        } else {
            SortOrder::Desc
        },
        where_filters,
        where_in_filters: Vec::new(),
        where_expr_filters,
        clip: reader.get_clip(),
        output: output_format_from_capnp(
            reader.get_output().unwrap_or(rpc::OutputFormat::Objects),
            reader.get_hash_prec(),
        ),
    })
}

pub(crate) fn fence_def_from_chan_request(
    reader: rpc::set_chan_request::Reader<'_>,
) -> Result<(String, GeofenceDef), capnp::Error> {
    let name = read_text(reader.get_name())?;
    let collection_and_query = match reader.get_search().which().map_err(not_in_schema_error)? {
        rpc::set_chan_request::search::WhichReader::Nearby(req) => {
            let (collection, query) = nearby_query_from_reader(req?)?;
            (
                collection,
                GeofenceQuery::Nearby {
                    lat: query.lat,
                    lon: query.lon,
                    meters: query.meters,
                    options: query.options,
                },
            )
        }
        rpc::set_chan_request::search::WhichReader::Within(req) => {
            let (collection, area, options) = area_query_from_within(req?)?;
            (collection, GeofenceQuery::Within { area, options })
        }
        rpc::set_chan_request::search::WhichReader::Intersects(req) => {
            let (collection, area, options) = area_query_from_intersects(req?)?;
            (collection, GeofenceQuery::Intersects { area, options })
        }
    };
    Ok((
        name,
        GeofenceDef {
            collection: collection_and_query.0,
            query: collection_and_query.1,
            detect: detect_list_from_reader(reader.get_detect()?)?,
            commands: command_list_from_reader(reader.get_commands()?)?,
        },
    ))
}

pub(crate) fn fence_def_from_hook_request(
    reader: rpc::set_hook_request::Reader<'_>,
) -> Result<(String, String, GeofenceDef), capnp::Error> {
    let name = read_text(reader.get_name())?;
    let endpoint = read_text(reader.get_endpoint())?;
    let collection_and_query = match reader.get_search().which().map_err(not_in_schema_error)? {
        rpc::set_hook_request::search::WhichReader::Nearby(req) => {
            let (collection, query) = nearby_query_from_reader(req?)?;
            (
                collection,
                GeofenceQuery::Nearby {
                    lat: query.lat,
                    lon: query.lon,
                    meters: query.meters,
                    options: query.options,
                },
            )
        }
        rpc::set_hook_request::search::WhichReader::Within(req) => {
            let (collection, area, options) = area_query_from_within(req?)?;
            (collection, GeofenceQuery::Within { area, options })
        }
        rpc::set_hook_request::search::WhichReader::Intersects(req) => {
            let (collection, area, options) = area_query_from_intersects(req?)?;
            (collection, GeofenceQuery::Intersects { area, options })
        }
    };
    Ok((
        name,
        endpoint,
        GeofenceDef {
            collection: collection_and_query.0,
            query: collection_and_query.1,
            detect: detect_list_from_reader(reader.get_detect()?)?,
            commands: command_list_from_reader(reader.get_commands()?)?,
        },
    ))
}

pub(crate) fn detect_list_from_reader(
    reader: capnp::enum_list::Reader<'_, rpc::DetectType>,
) -> Result<Vec<DetectType>, capnp::Error> {
    reader
        .iter()
        .map(|item| item.map(detect_from_capnp).map_err(not_in_schema_error))
        .collect()
}

pub(crate) fn command_list_from_reader(
    reader: capnp::text_list::Reader<'_>,
) -> Result<Vec<MutationCommand>, capnp::Error> {
    text_list_from_reader(reader)?
        .into_iter()
        .map(|item| {
            command_from_text(&item).ok_or_else(|| capnp_failed(format!("unknown command: {item}")))
        })
        .collect()
}

pub(crate) fn text_list_from_reader(
    reader: capnp::text_list::Reader<'_>,
) -> Result<Vec<String>, capnp::Error> {
    reader.iter().map(read_text).collect()
}

pub(crate) fn fill_text_list(mut builder: capnp::text_list::Builder<'_>, items: &[String]) {
    for (index, item) in items.iter().enumerate() {
        builder.set(index as u32, item.as_str());
    }
}

pub(crate) fn read_text(
    value: capnp::Result<capnp::text::Reader<'_>>,
) -> Result<String, capnp::Error> {
    Ok(value?.to_string()?)
}

pub(crate) fn output_format_from_capnp(value: rpc::OutputFormat, hash_prec: u8) -> OutputFormat {
    match value {
        rpc::OutputFormat::Objects => OutputFormat::Objects,
        rpc::OutputFormat::Points => OutputFormat::Points,
        rpc::OutputFormat::Bounds => OutputFormat::Bounds,
        rpc::OutputFormat::Hashes => OutputFormat::Hashes {
            precision: if hash_prec == 0 { 7 } else { hash_prec },
        },
        rpc::OutputFormat::Ids => OutputFormat::Ids,
        rpc::OutputFormat::Count => OutputFormat::Count,
    }
}

pub(crate) fn set_condition_from_capnp(value: rpc::SetCondition) -> SetCondition {
    match value {
        rpc::SetCondition::Always => SetCondition::Always,
        rpc::SetCondition::Nx => SetCondition::Nx,
        rpc::SetCondition::Xx => SetCondition::Xx,
    }
}

pub(crate) fn detect_from_capnp(value: rpc::DetectType) -> DetectType {
    match value {
        rpc::DetectType::Inside => DetectType::Inside,
        rpc::DetectType::Outside => DetectType::Outside,
        rpc::DetectType::Enter => DetectType::Enter,
        rpc::DetectType::Exit => DetectType::Exit,
        rpc::DetectType::Cross => DetectType::Cross,
        rpc::DetectType::Roam => DetectType::Roam,
    }
}

pub(crate) fn detect_to_capnp(value: DetectType) -> rpc::DetectType {
    match value {
        DetectType::Inside => rpc::DetectType::Inside,
        DetectType::Outside => rpc::DetectType::Outside,
        DetectType::Enter => rpc::DetectType::Enter,
        DetectType::Exit => rpc::DetectType::Exit,
        DetectType::Cross => rpc::DetectType::Cross,
        DetectType::Roam => rpc::DetectType::Roam,
    }
}

pub(crate) fn command_from_text(value: &str) -> Option<MutationCommand> {
    match value.to_ascii_lowercase().as_str() {
        "set" => Some(MutationCommand::Set),
        "del" => Some(MutationCommand::Del),
        "drop" => Some(MutationCommand::Drop),
        "fset" => Some(MutationCommand::Fset),
        _ => None,
    }
}

pub(crate) fn command_name(value: MutationCommand) -> &'static str {
    match value {
        MutationCommand::Set => "set",
        MutationCommand::Del => "del",
        MutationCommand::Drop => "drop",
        MutationCommand::Fset => "fset",
    }
}

pub(crate) fn empty_search_item() -> SearchItem {
    SearchItem {
        id: String::new(),
        object: Some(GeoType::String(String::new())),
        fields: Some(FieldMap::new()),
        distance_meters: None,
    }
}

pub(crate) fn default_glob(pattern: String) -> String {
    if pattern.is_empty() {
        "*".to_owned()
    } else {
        pattern
    }
}

pub(crate) fn json_string(value: &impl Serialize) -> Result<String, capnp::Error> {
    serde_json::to_string(value).map_err(capnp_failed)
}

pub(crate) fn not_in_schema_error(error: capnp::NotInSchema) -> capnp::Error {
    capnp_failed(error.to_string())
}

pub(crate) fn capnp_failed(error: impl ToString) -> capnp::Error {
    capnp::Error::failed(error.to_string())
}

pub(crate) fn unauthorized_error() -> capnp::Error {
    capnp::Error::failed("unauthorized".to_owned())
}

pub(crate) fn forbidden_error() -> capnp::Error {
    capnp::Error::failed("forbidden".to_owned())
}

#[cfg(test)]
mod tests {
    use capnp::message::Builder;

    use super::*;

    #[test]
    fn id_only_search_response_uses_flat_ids_list() {
        let response = SearchResults {
            results: vec![SearchItem {
                id: "truck-1".to_owned(),
                object: None,
                fields: None,
                distance_meters: None,
            }],
            cursor: 0,
            count: 1,
        };
        let mut message = Builder::new_default();
        fill_search_response(
            message.init_root::<rpc::search_response::Builder<'_>>(),
            &response,
        )
        .unwrap();

        let reader = message
            .get_root_as_reader::<rpc::search_response::Reader<'_>>()
            .unwrap();
        assert_eq!(reader.get_results().unwrap().len(), 0);
        let ids = reader.get_ids().unwrap();
        assert_eq!(ids.len(), 1);
        assert_eq!(ids.get(0).unwrap(), "truck-1");
    }

    #[test]
    fn search_item_from_object_fills_result_without_intermediate_item() {
        let mut fields = FieldMap::new();
        fields.insert("speed".to_owned(), FieldValue::Number(80.0));
        let object = Object {
            id: "truck-1".to_owned(),
            geo: GeoType::point(52.52, 13.405),
            fields,
            expires_at: None,
        };
        let mut message = Builder::new_default();
        fill_search_item_from_object(
            message.init_root::<rpc::search_result::Builder<'_>>(),
            &object,
        )
        .unwrap();

        let reader = message
            .get_root_as_reader::<rpc::search_result::Reader<'_>>()
            .unwrap();
        assert_eq!(reader.get_id().unwrap(), "truck-1");
        assert_eq!(reader.get_fields().unwrap().len(), 1);
    }
}
