use std::sync::Arc;

use latlng_auth::{AuthAction, AuthError};
use latlng_core::geo::BoundingBox;
use latlng_core::storage::StorageBackend;
use latlng_core::{FieldEntry, GetOptions};
use latlng_replication::ReplicationStatus;

use crate::codec::*;
use crate::rpc_state::LatLngRpc;
use crate::runtime::{
    apply_replication_to_server_info, rewrite_runtime_config, snapshot_replication_status,
    sync_effective_read_only, update_runtime_config,
};
use crate::streams::{GeofenceStreamRpc, ReplicationStreamRpc};
use crate::{geofence_stream, lat_lng, replication_stream, rpc, schema};

impl<S> lat_lng::Server for LatLngRpc<S>
where
    S: StorageBackend + Send + Sync + 'static,
{
    async fn set(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::SetParams,
        mut results: lat_lng::SetResults,
    ) -> Result<(), capnp::Error> {
        let req = set_request_from_reader(params.get()?.get_req()?)?;
        self.ensure_collection_action(AuthAction::ObjectsWrite, &req.collection)?;
        let stored = self.run_core_mutating(move |db| db.set(req)).await;
        let out = results.get();
        match stored {
            Ok(true) => fill_ok_response(out.init_resp(), true, ""),
            Ok(false) => fill_ok_response(out.init_resp(), false, "condition not met"),
            Err(error) => fill_ok_response(out.init_resp(), false, &error.to_string()),
        }
        Ok(())
    }

    async fn get(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::GetParams,
        mut results: lat_lng::GetResults,
    ) -> Result<(), capnp::Error> {
        let req = params.get()?.get_req()?;
        let collection = read_text(req.get_collection())?;
        let id = read_text(req.get_id())?;
        self.ensure_collection_action(AuthAction::ObjectsRead, &collection)?;
        let options = GetOptions {
            with_fields: req.get_with_fields(),
            output: output_format_from_capnp(
                req.get_output().unwrap_or(rpc::OutputFormat::Objects),
                req.get_hash_prec(),
            ),
        };
        let object = self
            .run_core_read(move |db| db.get(&collection, &id, options))
            .await;
        let mut out = results.get();
        match object {
            Ok(Some(object)) => {
                fill_search_item_from_object(out.reborrow().init_result(), &object)?;
                out.set_ok(true);
                out.set_error("");
            }
            Ok(None) => {
                fill_search_item(out.reborrow().init_result(), &empty_search_item(), false)?;
                out.set_ok(false);
                out.set_error("not found");
            }
            Err(error) => {
                fill_search_item(out.reborrow().init_result(), &empty_search_item(), false)?;
                out.set_ok(false);
                out.set_error(error.to_string());
            }
        }
        Ok(())
    }

    async fn del(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::DelParams,
        mut results: lat_lng::DelResults,
    ) -> Result<(), capnp::Error> {
        let params = params.get()?;
        let collection = read_text(params.get_collection())?;
        self.ensure_collection_action(AuthAction::ObjectsDelete, &collection)?;
        let id = read_text(params.get_id())?;
        let deleted = self
            .run_core_mutating(move |db| db.del(&collection, &id))
            .await;
        let out = results.get();
        match deleted {
            Ok(true) => fill_ok_response(out.init_resp(), true, ""),
            Ok(false) => fill_ok_response(out.init_resp(), false, "not found"),
            Err(error) => fill_ok_response(out.init_resp(), false, &error.to_string()),
        }
        Ok(())
    }

    async fn pdel(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::PdelParams,
        mut results: lat_lng::PdelResults,
    ) -> Result<(), capnp::Error> {
        let params = params.get()?;
        let collection = read_text(params.get_collection())?;
        self.ensure_collection_action(AuthAction::ObjectsDelete, &collection)?;
        let pattern = default_glob(read_text(params.get_pattern())?);
        let result = self
            .run_core_mutating(move |db| db.pdel(&collection, &pattern))
            .await;
        let out = results.get();
        match result {
            Ok(_) => fill_ok_response(out.init_resp(), true, ""),
            Err(error) => fill_ok_response(out.init_resp(), false, &error.to_string()),
        }
        Ok(())
    }

    async fn drop(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::DropParams,
        mut results: lat_lng::DropResults,
    ) -> Result<(), capnp::Error> {
        let collection = read_text(params.get()?.get_collection())?;
        self.ensure_collection_action(AuthAction::CollectionsDelete, &collection)?;
        let dropped = self
            .run_core_mutating(move |db| db.drop_collection(&collection))
            .await;
        let out = results.get();
        match dropped {
            Ok(true) => fill_ok_response(out.init_resp(), true, ""),
            Ok(false) => fill_ok_response(out.init_resp(), false, "not found"),
            Err(error) => fill_ok_response(out.init_resp(), false, &error.to_string()),
        }
        Ok(())
    }

    async fn rename(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::RenameParams,
        mut results: lat_lng::RenameResults,
    ) -> Result<(), capnp::Error> {
        let params = params.get()?;
        let collection = read_text(params.get_collection())?;
        let new_name = read_text(params.get_newname())?;
        self.ensure_collection_action(AuthAction::CollectionsDelete, &collection)?;
        self.ensure_collection_action(AuthAction::CollectionsCreate, &new_name)?;
        let result = self
            .run_core_mutating(move |db| db.rename(&collection, &new_name))
            .await;
        let out = results.get();
        match result {
            Ok(()) => fill_ok_response(out.init_resp(), true, ""),
            Err(error) => fill_ok_response(out.init_resp(), false, &error.to_string()),
        }
        Ok(())
    }

    async fn renamenx(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::RenamenxParams,
        mut results: lat_lng::RenamenxResults,
    ) -> Result<(), capnp::Error> {
        let params = params.get()?;
        let collection = read_text(params.get_collection())?;
        let new_name = read_text(params.get_newname())?;
        self.ensure_collection_action(AuthAction::CollectionsDelete, &collection)?;
        self.ensure_collection_action(AuthAction::CollectionsCreate, &new_name)?;
        let result = self
            .run_core_mutating(move |db| db.renamenx(&collection, &new_name))
            .await;
        let out = results.get();
        match result {
            Ok(true) => fill_ok_response(out.init_resp(), true, ""),
            Ok(false) => fill_ok_response(out.init_resp(), false, "destination already exists"),
            Err(error) => fill_ok_response(out.init_resp(), false, &error.to_string()),
        }
        Ok(())
    }

    async fn fset(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::FsetParams,
        mut results: lat_lng::FsetResults,
    ) -> Result<(), capnp::Error> {
        let params = params.get()?;
        let collection = read_text(params.get_collection())?;
        self.ensure_collection_action(AuthAction::ObjectsWrite, &collection)?;
        let id = read_text(params.get_id())?;
        let fields = field_entries_from_list(params.get_fields()?)?;
        let xx = params.get_xx();
        let updated = self
            .run_core_mutating(move |db| db.fset(&collection, &id, &fields, xx))
            .await;
        let out = results.get();
        match updated {
            Ok(true) => fill_ok_response(out.init_resp(), true, ""),
            Ok(false) => fill_ok_response(out.init_resp(), false, "not found"),
            Err(error) => fill_ok_response(out.init_resp(), false, &error.to_string()),
        }
        Ok(())
    }

    async fn fget(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::FgetParams,
        mut results: lat_lng::FgetResults,
    ) -> Result<(), capnp::Error> {
        let params = params.get()?;
        let collection = read_text(params.get_collection())?;
        self.ensure_collection_action(AuthAction::ObjectsRead, &collection)?;
        let id = read_text(params.get_id())?;
        let field = read_text(params.get_field())?;
        let query_field = field.clone();
        let value = self
            .run_core_read(move |db| db.fget(&collection, &id, &query_field))
            .await;
        let mut out = results.get();
        match value {
            Ok(Some(value)) => {
                fill_field_entry(
                    out.reborrow().init_value(),
                    &FieldEntry { name: field, value },
                );
                out.set_ok(true);
            }
            Ok(None) => out.set_ok(false),
            Err(_) => out.set_ok(false),
        }
        Ok(())
    }

    async fn expire(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::ExpireParams,
        mut results: lat_lng::ExpireResults,
    ) -> Result<(), capnp::Error> {
        let params = params.get()?;
        let collection = read_text(params.get_collection())?;
        self.ensure_collection_action(AuthAction::ObjectsWrite, &collection)?;
        let id = read_text(params.get_id())?;
        let seconds = params.get_seconds();
        let result = self
            .run_core_mutating(move |db| db.expire(&collection, &id, seconds))
            .await;
        let out = results.get();
        match result {
            Ok(()) => fill_ok_response(out.init_resp(), true, ""),
            Err(error) => fill_ok_response(out.init_resp(), false, &error.to_string()),
        }
        Ok(())
    }

    async fn persist(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::PersistParams,
        mut results: lat_lng::PersistResults,
    ) -> Result<(), capnp::Error> {
        let params = params.get()?;
        let collection = read_text(params.get_collection())?;
        self.ensure_collection_action(AuthAction::ObjectsWrite, &collection)?;
        let id = read_text(params.get_id())?;
        let result = self
            .run_core_mutating(move |db| db.persist(&collection, &id))
            .await;
        let out = results.get();
        match result {
            Ok(()) => fill_ok_response(out.init_resp(), true, ""),
            Err(error) => fill_ok_response(out.init_resp(), false, &error.to_string()),
        }
        Ok(())
    }

    async fn ttl(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::TtlParams,
        mut results: lat_lng::TtlResults,
    ) -> Result<(), capnp::Error> {
        let params = params.get()?;
        let collection = read_text(params.get_collection())?;
        self.ensure_collection_action(AuthAction::ObjectsRead, &collection)?;
        let id = read_text(params.get_id())?;
        let ttl = self.run_core_read(move |db| db.ttl(&collection, &id)).await;
        let mut out = results.get();
        match ttl {
            Ok(Some(seconds)) => {
                out.set_seconds(seconds);
                out.set_ok(true);
            }
            Ok(None) => {
                out.set_seconds(-1);
                out.set_ok(false);
            }
            Err(_) => {
                out.set_seconds(-1);
                out.set_ok(false);
            }
        }
        Ok(())
    }

    async fn exists(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::ExistsParams,
        mut results: lat_lng::ExistsResults,
    ) -> Result<(), capnp::Error> {
        let params = params.get()?;
        let collection = read_text(params.get_collection())?;
        self.ensure_collection_action(AuthAction::ObjectsRead, &collection)?;
        let id = read_text(params.get_id())?;
        let exists = self
            .run_core_read(move |db| db.exists(&collection, &id))
            .await
            .unwrap_or(false);
        results.get().set_exists(exists);
        Ok(())
    }

    async fn fexists(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::FexistsParams,
        mut results: lat_lng::FexistsResults,
    ) -> Result<(), capnp::Error> {
        let params = params.get()?;
        let collection = read_text(params.get_collection())?;
        self.ensure_collection_action(AuthAction::ObjectsRead, &collection)?;
        let id = read_text(params.get_id())?;
        let field = read_text(params.get_field())?;
        let exists = self
            .run_core_read(move |db| db.fexists(&collection, &id, &field))
            .await
            .unwrap_or(false);
        results.get().set_exists(exists);
        Ok(())
    }

    async fn bounds(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::BoundsParams,
        mut results: lat_lng::BoundsResults,
    ) -> Result<(), capnp::Error> {
        let collection = read_text(params.get()?.get_collection())?;
        self.ensure_collection_action(AuthAction::CollectionsInspect, &collection)?;
        let bounds = self.run_core_read(move |db| db.bounds(&collection)).await;
        let out = results.get();
        let mut resp = out.init_resp();
        match bounds {
            Ok(Some(bounds)) => {
                resp.set_ok(true);
                fill_bounds(resp.init_bounds(), &bounds);
            }
            _ => {
                resp.set_ok(false);
                fill_bounds(resp.init_bounds(), &BoundingBox::new(0.0, 0.0, 0.0, 0.0));
            }
        }
        Ok(())
    }

    async fn collections(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::CollectionsParams,
        mut results: lat_lng::CollectionsResults,
    ) -> Result<(), capnp::Error> {
        let principal = self.ensure_any_collection_permission(AuthAction::CollectionsList)?;
        let pattern = default_glob(read_text(params.get()?.get_pattern())?);
        let names = self
            .run_core_read(move |db| db.collections(&pattern))
            .await
            .unwrap_or_default();
        let names = names
            .into_iter()
            .filter(|name| principal.can_view_collection(name))
            .collect::<Vec<_>>();
        let out = results.get();
        fill_text_list(out.init_names(names.len() as u32), &names);
        Ok(())
    }

    async fn stats(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::StatsParams,
        mut results: lat_lng::StatsResults,
    ) -> Result<(), capnp::Error> {
        let principal = self.ensure_authenticated()?;
        let requested = text_list_from_reader(params.get()?.get_collections()?)?;
        if requested.is_empty() {
            if !principal.is_admin()
                && !principal.any_collection_permission(AuthAction::CollectionsList)
            {
                return Err(forbidden_error());
            }
        } else {
            for collection in &requested {
                self.ensure_collection_action(AuthAction::CollectionsInspect, collection)?;
            }
        }
        let stats = self
            .run_core_read(move |db| {
                let names = if requested.is_empty() {
                    db.collections("*")?
                } else {
                    requested
                };
                let refs = names.iter().map(String::as_str).collect::<Vec<_>>();
                db.stats(&refs)
            })
            .await
            .unwrap_or_default();
        let rendered = stats
            .iter()
            .filter(|entry| principal.allows(AuthAction::CollectionsInspect, &entry.name))
            .map(json_string)
            .collect::<Result<Vec<_>, _>>()?;
        let out = results.get();
        fill_text_list(out.init_stats(rendered.len() as u32), &rendered);
        Ok(())
    }

    async fn jset(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::JsetParams,
        mut results: lat_lng::JsetResults,
    ) -> Result<(), capnp::Error> {
        let params = params.get()?;
        let collection = read_text(params.get_collection())?;
        self.ensure_collection_action(AuthAction::ObjectsWrite, &collection)?;
        let id = read_text(params.get_id())?;
        let path = read_text(params.get_path())?;
        let value = read_text(params.get_value())?;
        let raw = params.get_raw();
        let result = self
            .run_core_mutating(move |db| db.jset(&collection, &id, &path, &value, raw))
            .await;
        let out = results.get();
        match result {
            Ok(()) => fill_ok_response(out.init_resp(), true, ""),
            Err(error) => fill_ok_response(out.init_resp(), false, &error.to_string()),
        }
        Ok(())
    }

    async fn jget(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::JgetParams,
        mut results: lat_lng::JgetResults,
    ) -> Result<(), capnp::Error> {
        let params = params.get()?;
        let collection = read_text(params.get_collection())?;
        self.ensure_collection_action(AuthAction::ObjectsRead, &collection)?;
        let id = read_text(params.get_id())?;
        let path = read_text(params.get_path())?;
        let value = self
            .run_core_read(move |db| db.jget(&collection, &id, &path))
            .await;
        let mut out = results.get();
        match value {
            Ok(Some(value)) => {
                out.set_value(&value);
                out.set_ok(true);
            }
            _ => {
                out.set_value("");
                out.set_ok(false);
            }
        }
        Ok(())
    }

    async fn jdel(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::JdelParams,
        mut results: lat_lng::JdelResults,
    ) -> Result<(), capnp::Error> {
        let params = params.get()?;
        let collection = read_text(params.get_collection())?;
        self.ensure_collection_action(AuthAction::ObjectsWrite, &collection)?;
        let id = read_text(params.get_id())?;
        let path = read_text(params.get_path())?;
        let deleted = self
            .run_core_mutating(move |db| db.jdel(&collection, &id, &path))
            .await;
        let out = results.get();
        match deleted {
            Ok(true) => fill_ok_response(out.init_resp(), true, ""),
            Ok(false) => fill_ok_response(out.init_resp(), false, "path not found"),
            Err(error) => fill_ok_response(out.init_resp(), false, &error.to_string()),
        }
        Ok(())
    }

    async fn nearby(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::NearbyParams,
        mut results: lat_lng::NearbyResults,
    ) -> Result<(), capnp::Error> {
        let req = params.get()?.get_req()?;
        let (collection, query) = nearby_query_from_reader(req)?;
        self.ensure_collection_action(AuthAction::QueriesRead, &collection)?;
        let response = self
            .run_core_read(move |db| db.nearby(&collection, query))
            .await;
        let out = results.get();
        match response {
            Ok(response) => fill_search_response(out.init_resp(), &response)?,
            Err(error) => fill_search_error(out.init_resp(), &error.to_string()),
        }
        Ok(())
    }

    async fn within(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::WithinParams,
        mut results: lat_lng::WithinResults,
    ) -> Result<(), capnp::Error> {
        let req = params.get()?.get_req()?;
        let (collection, area, options) = area_query_from_within(req)?;
        self.ensure_collection_action(AuthAction::QueriesRead, &collection)?;
        let response = self
            .run_core_read(move |db| db.within(&collection, area, options))
            .await;
        let out = results.get();
        match response {
            Ok(response) => fill_search_response(out.init_resp(), &response)?,
            Err(error) => fill_search_error(out.init_resp(), &error.to_string()),
        }
        Ok(())
    }

    async fn intersects(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::IntersectsParams,
        mut results: lat_lng::IntersectsResults,
    ) -> Result<(), capnp::Error> {
        let req = params.get()?.get_req()?;
        let (collection, area, options) = area_query_from_intersects(req)?;
        self.ensure_collection_action(AuthAction::QueriesRead, &collection)?;
        let response = self
            .run_core_read(move |db| db.intersects(&collection, area, options))
            .await;
        let out = results.get();
        match response {
            Ok(response) => fill_search_response(out.init_resp(), &response)?,
            Err(error) => fill_search_error(out.init_resp(), &error.to_string()),
        }
        Ok(())
    }

    async fn scan(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::ScanParams,
        mut results: lat_lng::ScanResults,
    ) -> Result<(), capnp::Error> {
        let req = params.get()?.get_req()?;
        let collection = read_text(req.get_collection())?;
        self.ensure_collection_action(AuthAction::QueriesRead, &collection)?;
        let options = search_options_from_reader(req.get_options()?)?;
        let response = self
            .run_core_read(move |db| db.scan(&collection, options))
            .await;
        let out = results.get();
        match response {
            Ok(response) => fill_search_response(out.init_resp(), &response)?,
            Err(error) => fill_search_error(out.init_resp(), &error.to_string()),
        }
        Ok(())
    }

    async fn search(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::SearchParams,
        mut results: lat_lng::SearchResults,
    ) -> Result<(), capnp::Error> {
        let req = params.get()?.get_req()?;
        let collection = read_text(req.get_collection())?;
        self.ensure_collection_action(AuthAction::QueriesRead, &collection)?;
        let options = search_options_from_reader(req.get_options()?)?;
        let response = self
            .run_core_read(move |db| db.search(&collection, options))
            .await;
        let out = results.get();
        match response {
            Ok(response) => fill_search_response(out.init_resp(), &response)?,
            Err(error) => fill_search_error(out.init_resp(), &error.to_string()),
        }
        Ok(())
    }

    async fn setchan(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::SetchanParams,
        mut results: lat_lng::SetchanResults,
    ) -> Result<(), capnp::Error> {
        let req = params.get()?.get_req()?;
        let (name, def) = fence_def_from_chan_request(req)?;
        self.ensure_collection_action(AuthAction::ChannelsManage, &def.collection)?;
        let result = self
            .run_core_mutating(move |db| db.setchan(&name, def))
            .await;
        let out = results.get();
        match result {
            Ok(()) => fill_ok_response(out.init_resp(), true, ""),
            Err(error) => fill_ok_response(out.init_resp(), false, &error.to_string()),
        }
        Ok(())
    }

    async fn delchan(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::DelchanParams,
        mut results: lat_lng::DelchanResults,
    ) -> Result<(), capnp::Error> {
        let name = read_text(params.get()?.get_name())?;
        if let Some(collection) = self
            .run_value({
                let name = name.clone();
                move |db| db.channel_def(&name).map(|channel| channel.def.collection)
            })
            .await?
        {
            self.ensure_collection_action(AuthAction::ChannelsManage, &collection)?;
        }
        let deleted = self.run_core_mutating(move |db| db.delchan(&name)).await;
        let out = results.get();
        match deleted {
            Ok(true) => fill_ok_response(out.init_resp(), true, ""),
            Ok(false) => fill_ok_response(out.init_resp(), false, "not found"),
            Err(error) => fill_ok_response(out.init_resp(), false, &error.to_string()),
        }
        Ok(())
    }

    async fn pdelchan(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::PdelchanParams,
        mut results: lat_lng::PdelchanResults,
    ) -> Result<(), capnp::Error> {
        let principal = self.ensure_authenticated()?;
        let pattern = default_glob(read_text(params.get()?.get_pattern())?);
        let channel_defs = self
            .run_core_read({
                let pattern = pattern.clone();
                move |db| {
                    db.chans(&pattern).map(|names| {
                        names
                            .into_iter()
                            .filter_map(|name| db.channel_def(&name))
                            .collect::<Vec<_>>()
                    })
                }
            })
            .await?;
        if channel_defs
            .iter()
            .any(|channel| !principal.allows(AuthAction::ChannelsManage, &channel.def.collection))
        {
            return Err(forbidden_error());
        }
        let result = self
            .run_core_mutating(move |db| db.pdelchan(&pattern))
            .await;
        let out = results.get();
        match result {
            Ok(_) => fill_ok_response(out.init_resp(), true, ""),
            Err(error) => fill_ok_response(out.init_resp(), false, &error.to_string()),
        }
        Ok(())
    }

    async fn chans(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::ChansParams,
        mut results: lat_lng::ChansResults,
    ) -> Result<(), capnp::Error> {
        let principal = self.ensure_authenticated()?;
        let pattern = default_glob(read_text(params.get()?.get_pattern())?);
        let channels = self
            .run_core_read(move |db| {
                db.chans(&pattern).map(|names| {
                    names
                        .into_iter()
                        .filter_map(|name| db.channel_def(&name).map(|def| (name, def)))
                        .collect::<Vec<_>>()
                })
            })
            .await
            .unwrap_or_default();
        let channels = channels
            .into_iter()
            .filter_map(|(name, def)| {
                principal
                    .allows(AuthAction::ChannelsManage, &def.def.collection)
                    .then_some(name)
            })
            .collect::<Vec<_>>();
        let out = results.get();
        fill_text_list(out.init_channels(channels.len() as u32), &channels);
        Ok(())
    }

    async fn subscribe(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::SubscribeParams,
        mut results: lat_lng::SubscribeResults,
    ) -> Result<(), capnp::Error> {
        let principal = self.ensure_authenticated()?;
        let channels = text_list_from_reader(params.get()?.get_channels()?)?;
        let channel_defs = self
            .run_core_read({
                let channels = channels.clone();
                move |db| {
                    Ok(channels
                        .iter()
                        .filter_map(|name| db.channel_def(name))
                        .collect::<Vec<_>>())
                }
            })
            .await?;
        if channel_defs.iter().any(|channel| {
            !principal.allows(AuthAction::SubscriptionsRead, &channel.def.collection)
        }) {
            return Err(forbidden_error());
        }
        let receiver = self
            .run_value(move |db| {
                let refs = channels.iter().map(String::as_str).collect::<Vec<_>>();
                db.subscribe(&refs)
            })
            .await?;
        let stream: geofence_stream::Client =
            capnp_rpc::new_client(GeofenceStreamRpc::new(receiver, principal));
        results.get().set_stream(stream);
        Ok(())
    }

    async fn psubscribe(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::PsubscribeParams,
        mut results: lat_lng::PsubscribeResults,
    ) -> Result<(), capnp::Error> {
        let principal = self.ensure_any_collection_permission(AuthAction::SubscriptionsRead)?;
        let patterns = text_list_from_reader(params.get()?.get_patterns()?)?;
        let receiver = self
            .run_value(move |db| {
                let refs = patterns.iter().map(String::as_str).collect::<Vec<_>>();
                db.psubscribe(&refs)
            })
            .await?;
        let stream: geofence_stream::Client =
            capnp_rpc::new_client(GeofenceStreamRpc::new(receiver, principal));
        results.get().set_stream(stream);
        Ok(())
    }

    async fn sethook(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::SethookParams,
        mut results: lat_lng::SethookResults,
    ) -> Result<(), capnp::Error> {
        let req = params.get()?.get_req()?;
        let (name, endpoint, def) = fence_def_from_hook_request(req)?;
        self.ensure_collection_action(AuthAction::HooksManage, &def.collection)?;
        let result = self
            .run_core_mutating(move |db| db.sethook(&name, &endpoint, def))
            .await;
        let out = results.get();
        match result {
            Ok(()) => fill_ok_response(out.init_resp(), true, ""),
            Err(error) => fill_ok_response(out.init_resp(), false, &error.to_string()),
        }
        Ok(())
    }

    async fn delhook(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::DelhookParams,
        mut results: lat_lng::DelhookResults,
    ) -> Result<(), capnp::Error> {
        let name = read_text(params.get()?.get_name())?;
        if let Some(collection) = self
            .run_value({
                let name = name.clone();
                move |db| db.hook_def(&name).map(|hook| hook.def.collection)
            })
            .await?
        {
            self.ensure_collection_action(AuthAction::HooksManage, &collection)?;
        }
        let deleted = self.run_core_mutating(move |db| db.delhook(&name)).await;
        let out = results.get();
        match deleted {
            Ok(true) => fill_ok_response(out.init_resp(), true, ""),
            Ok(false) => fill_ok_response(out.init_resp(), false, "not found"),
            Err(error) => fill_ok_response(out.init_resp(), false, &error.to_string()),
        }
        Ok(())
    }

    async fn pdelhook(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::PdelhookParams,
        mut results: lat_lng::PdelhookResults,
    ) -> Result<(), capnp::Error> {
        let principal = self.ensure_authenticated()?;
        let pattern = default_glob(read_text(params.get()?.get_pattern())?);
        let hooks = self
            .run_core_read({
                let pattern = pattern.clone();
                move |db| db.hooks(&pattern)
            })
            .await
            .unwrap_or_default();
        if hooks
            .iter()
            .any(|hook| !principal.allows(AuthAction::HooksManage, &hook.collection))
        {
            return Err(forbidden_error());
        }
        let result = self
            .run_core_mutating(move |db| db.pdelhook(&pattern))
            .await;
        let out = results.get();
        match result {
            Ok(_) => fill_ok_response(out.init_resp(), true, ""),
            Err(error) => fill_ok_response(out.init_resp(), false, &error.to_string()),
        }
        Ok(())
    }

    async fn hooks(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::HooksParams,
        mut results: lat_lng::HooksResults,
    ) -> Result<(), capnp::Error> {
        let principal = self.ensure_authenticated()?;
        let pattern = default_glob(read_text(params.get()?.get_pattern())?);
        let hooks = self
            .run_core_read(move |db| db.hooks(&pattern))
            .await
            .unwrap_or_default();
        let rendered = hooks
            .iter()
            .filter(|hook| principal.allows(AuthAction::HooksManage, &hook.collection))
            .map(json_string)
            .collect::<Result<Vec<_>, _>>()?;
        let out = results.get();
        fill_text_list(out.init_hooks(rendered.len() as u32), &rendered);
        Ok(())
    }

    async fn ping(
        self: capnp::capability::Rc<Self>,
        _: lat_lng::PingParams,
        mut results: lat_lng::PingResults,
    ) -> Result<(), capnp::Error> {
        self.ensure_authenticated()?;
        fill_ok_response(results.get().init_resp(), true, "");
        Ok(())
    }

    async fn server(
        self: capnp::capability::Rc<Self>,
        _: lat_lng::ServerParams,
        mut results: lat_lng::ServerResults,
    ) -> Result<(), capnp::Error> {
        self.ensure_global_action(AuthAction::AdminAll)?;
        let mut info = self.run_value(|db| db.server_info()).await?;
        apply_replication_to_server_info(
            &mut info,
            self.replication_status
                .as_ref()
                .map(snapshot_replication_status)
                .as_ref(),
        );
        fill_server_info(results.get().init_info(), &info);
        Ok(())
    }

    async fn info(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::InfoParams,
        mut results: lat_lng::InfoResults,
    ) -> Result<(), capnp::Error> {
        self.ensure_global_action(AuthAction::AdminAll)?;
        let section = read_text(params.get()?.get_section())?;
        let mut server_info = self.run_value(|db| db.server_info()).await?;
        apply_replication_to_server_info(
            &mut server_info,
            self.replication_status
                .as_ref()
                .map(snapshot_replication_status)
                .as_ref(),
        );
        let value = match section.as_str() {
            "" | "server" => json_string(&server_info)?,
            "schema" => schema::SCHEMA_TEXT.to_owned(),
            _ => json_string(&serde_json::json!({
                "server": server_info
            }))?,
        };
        results.get().set_info(&value);
        Ok(())
    }

    async fn healthz(
        self: capnp::capability::Rc<Self>,
        _: lat_lng::HealthzParams,
        mut results: lat_lng::HealthzResults,
    ) -> Result<(), capnp::Error> {
        self.ensure_authenticated()?;
        self.ensure_queries_allowed()?;
        fill_ok_response(results.get().init_resp(), true, "");
        Ok(())
    }

    async fn config_get(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::ConfigGetParams,
        mut results: lat_lng::ConfigGetResults,
    ) -> Result<(), capnp::Error> {
        self.ensure_global_action(AuthAction::AdminAll)?;
        let name = read_text(params.get()?.get_name())?;
        let value = {
            match name.as_str() {
                "readonly" => {
                    self.run_core(|db| {
                        Ok(db
                            .config()
                            .read()
                            .map(|guard| guard.read_only.to_string())
                            .unwrap_or_else(|poisoned| poisoned.into_inner().read_only.to_string()))
                    })
                    .await?
                }
                _ => String::new(),
            }
        };
        results.get().set_value(&value);
        Ok(())
    }

    async fn config_set(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::ConfigSetParams,
        mut results: lat_lng::ConfigSetResults,
    ) -> Result<(), capnp::Error> {
        self.ensure_global_action(AuthAction::AdminAll)?;
        let params = params.get()?;
        let name = read_text(params.get_name())?;
        let value = read_text(params.get_value())?;
        let out = results.get();
        match name.as_str() {
            "readonly" => {
                let enabled = matches!(value.as_str(), "true" | "yes" | "1");
                update_runtime_config(self.runtime_config.as_ref(), |config| {
                    config.read_only = enabled
                });
                sync_effective_read_only(
                    &self.executor,
                    self.runtime_config.as_ref(),
                    self.replication_status.as_ref(),
                )
                .await?;
                fill_ok_response(out.init_resp(), true, "");
            }
            _ => fill_ok_response(out.init_resp(), false, "unknown config key"),
        }
        Ok(())
    }

    async fn config_rewrite(
        self: capnp::capability::Rc<Self>,
        _: lat_lng::ConfigRewriteParams,
        mut results: lat_lng::ConfigRewriteResults,
    ) -> Result<(), capnp::Error> {
        self.ensure_global_action(AuthAction::AdminAll)?;
        match rewrite_runtime_config(self.runtime_config.as_ref()) {
            Ok(()) => fill_ok_response(results.get().init_resp(), true, ""),
            Err(error) => fill_ok_response(results.get().init_resp(), false, &error.to_string()),
        }
        Ok(())
    }

    async fn flushdb(
        self: capnp::capability::Rc<Self>,
        _: lat_lng::FlushdbParams,
        mut results: lat_lng::FlushdbResults,
    ) -> Result<(), capnp::Error> {
        self.ensure_global_action(AuthAction::AdminAll)?;
        let result: Result<(), capnp::Error> = if let Some(coordinator) = &self.flushdb_coordinator
        {
            coordinator
                .flushdb()
                .await
                .map_err(|error| capnp::Error::failed(error.to_string()))
        } else {
            self.run_core_mutating(|db| db.flushdb()).await
        };
        if result.is_ok()
            && let Some(notify) = &self.outbox_notify
        {
            notify.notify_waiters();
        }
        if result.is_ok()
            && let Some(notify) = &self.replication_notify
        {
            notify.notify_waiters();
        }
        let out = results.get();
        match result {
            Ok(()) => fill_ok_response(out.init_resp(), true, ""),
            Err(error) => fill_ok_response(out.init_resp(), false, &error.to_string()),
        }
        Ok(())
    }

    async fn gc(
        self: capnp::capability::Rc<Self>,
        _: lat_lng::GcParams,
        mut results: lat_lng::GcResults,
    ) -> Result<(), capnp::Error> {
        self.ensure_global_action(AuthAction::AdminAll)?;
        self.run_core(|db| {
            db.gc();
            Ok(())
        })
        .await?;
        fill_ok_response(results.get().init_resp(), true, "");
        Ok(())
    }

    async fn readonly(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::ReadonlyParams,
        mut results: lat_lng::ReadonlyResults,
    ) -> Result<(), capnp::Error> {
        self.ensure_global_action(AuthAction::AdminAll)?;
        let enabled = params.get()?.get_enabled();
        update_runtime_config(self.runtime_config.as_ref(), |config| {
            config.read_only = enabled
        });
        sync_effective_read_only(
            &self.executor,
            self.runtime_config.as_ref(),
            self.replication_status.as_ref(),
        )
        .await?;
        fill_ok_response(results.get().init_resp(), true, "");
        Ok(())
    }

    async fn auth(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::AuthParams,
        mut results: lat_lng::AuthResults,
    ) -> Result<(), capnp::Error> {
        let token = read_text(params.get()?.get_token())?;
        let out = results.get();
        match self.auth.authenticate(Some(&token)).await {
            Ok(principal) => {
                self.principal.replace(Some(principal));
                fill_ok_response(out.init_resp(), true, "");
            }
            Err(AuthError::Unauthorized) => {
                self.principal.replace(None);
                fill_ok_response(out.init_resp(), false, "unauthorized");
            }
            Err(error) => {
                self.principal.replace(None);
                fill_ok_response(out.init_resp(), false, &error.to_string());
            }
        }
        Ok(())
    }

    async fn timeout(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::TimeoutParams,
        mut results: lat_lng::TimeoutResults,
    ) -> Result<(), capnp::Error> {
        self.ensure_global_action(AuthAction::AdminAll)?;
        let params = params.get()?;
        let command = read_text(params.get_command())?;
        let seconds = params.get_seconds();
        if command.trim().is_empty() {
            fill_ok_response(results.get().init_resp(), false, "missing command");
            return Ok(());
        }
        if seconds <= 0.0 {
            let command_for_db = command.clone();
            self.run_core(move |db| {
                match db.config().write() {
                    Ok(mut guard) => guard.clear_timeout(&command_for_db),
                    Err(poisoned) => poisoned.into_inner().clear_timeout(&command_for_db),
                }
                Ok(())
            })
            .await?;
            update_runtime_config(self.runtime_config.as_ref(), |config| {
                config.clear_timeout(&command);
            });
        } else {
            let command_for_db = command.clone();
            self.run_core(move |db| {
                db.set_timeout(&command_for_db, seconds);
                Ok(())
            })
            .await?;
            update_runtime_config(self.runtime_config.as_ref(), |config| {
                config.set_timeout(&command, seconds);
            });
        }
        fill_ok_response(results.get().init_resp(), true, "");
        Ok(())
    }

    async fn role(
        self: capnp::capability::Rc<Self>,
        _: lat_lng::RoleParams,
        mut results: lat_lng::RoleResults,
    ) -> Result<(), capnp::Error> {
        self.ensure_global_action(AuthAction::AdminAll)?;
        let info = self
            .replication_status
            .as_ref()
            .map(snapshot_replication_status)
            .unwrap_or_else(|| ReplicationStatus::leader(String::new()));
        let mut out = results.get();
        out.set_role(match info.role {
            latlng_replication::ReplicationRole::Leader => "leader",
            latlng_replication::ReplicationRole::Follower => "follower",
        });
        out.set_info(&json_string(&info)?);
        Ok(())
    }

    async fn replication_info(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::ReplicationInfoParams,
        mut results: lat_lng::ReplicationInfoResults,
    ) -> Result<(), capnp::Error> {
        let credential = read_text(params.get()?.get_credential())?;
        self.ensure_replication_authorized(&credential)?;
        let mut server_info = self.run_value(|db| db.server_info()).await?;
        apply_replication_to_server_info(
            &mut server_info,
            self.replication_status
                .as_ref()
                .map(snapshot_replication_status)
                .as_ref(),
        );
        let mut out = results.get();
        out.set_ok(true);
        out.set_error("");
        fill_replication_info(out.init_info(), &server_info);
        Ok(())
    }

    async fn replication_checksum(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::ReplicationChecksumParams,
        mut results: lat_lng::ReplicationChecksumResults,
    ) -> Result<(), capnp::Error> {
        let params = params.get()?;
        let credential = read_text(params.get_credential())?;
        self.ensure_replication_authorized(&credential)?;
        let from = params.get_from();
        let to = params.get_to();
        let mut out = results.get();
        match self.run_core(move |db| db.checksum_range(from, to)).await {
            Ok(checksum) => {
                out.set_ok(true);
                out.set_error("");
                out.set_checksum(&checksum);
            }
            Err(error) => {
                out.set_ok(false);
                out.set_error(error.to_string());
                out.set_checksum(&[]);
            }
        }
        Ok(())
    }

    async fn replication_stream(
        self: capnp::capability::Rc<Self>,
        params: lat_lng::ReplicationStreamParams,
        mut results: lat_lng::ReplicationStreamResults,
    ) -> Result<(), capnp::Error> {
        let params = params.get()?;
        let credential = read_text(params.get_credential())?;
        self.ensure_replication_authorized(&credential)?;
        let notify = self
            .replication_notify
            .as_ref()
            .map(Arc::clone)
            .ok_or_else(|| capnp_failed("replication notifier is not attached"))?;
        let stream: replication_stream::Client = capnp_rpc::new_client(ReplicationStreamRpc::new(
            self.executor.clone(),
            params.get_after_sequence(),
            usize::try_from(params.get_batch_size()).unwrap_or(1),
            notify,
        ));
        let mut out = results.get();
        out.set_ok(true);
        out.set_error("");
        out.set_stream(stream);
        Ok(())
    }

    async fn aofshrink(
        self: capnp::capability::Rc<Self>,
        _: lat_lng::AofshrinkParams,
        mut results: lat_lng::AofshrinkResults,
    ) -> Result<(), capnp::Error> {
        self.ensure_global_action(AuthAction::AdminAll)?;
        let result = self.run_core(|db| db.aofshrink()).await;
        if result.is_ok()
            && let Some(notify) = &self.replication_notify
        {
            notify.notify_waiters();
        }
        let out = results.get();
        match result {
            Ok(_) => fill_ok_response(out.init_resp(), true, ""),
            Err(error) => fill_ok_response(out.init_resp(), false, &error.to_string()),
        }
        Ok(())
    }
}
