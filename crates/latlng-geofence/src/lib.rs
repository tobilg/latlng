#![forbid(unsafe_code)]

use geo::LineString;
use geo::algorithm::intersects::Intersects;
use glob_match::glob_match;
use hashbrown::{HashMap, HashSet};
use latlng_geo::{Area, GeoError, GeoType, Object, haversine_distance_meters};
use latlng_index::{IndexError, SearchOptions, matches_candidate};
use latlng_platform::{
    NativePlatform, NativeWakeHandle, Platform, PlatformReceiver, PlatformSender,
};
use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicBool;
use thiserror::Error;

pub use latlng_platform as platform;

pub const DEFAULT_SUBSCRIBER_QUEUE_CAPACITY: usize = 4_096;

pub struct GeofenceEventReceiver<P: Platform> {
    receiver: P::Receiver<GeofenceEvent>,
    generation: P::Shared<P::RwLock<u64>>,
}

impl<P: Platform> GeofenceEventReceiver<P> {
    fn new(receiver: P::Receiver<GeofenceEvent>, generation: P::Shared<P::RwLock<u64>>) -> Self {
        Self {
            receiver,
            generation,
        }
    }

    pub fn try_recv(&mut self) -> Option<GeofenceEvent> {
        loop {
            let current_generation = *P::read(&*self.generation);
            let event = self.receiver.try_recv()?;
            if event.generation >= current_generation {
                return Some(event);
            }
        }
    }
}

impl GeofenceEventReceiver<NativePlatform> {
    pub fn wake_handle(&self) -> NativeWakeHandle<GeofenceEvent> {
        self.receiver.wake_handle()
    }

    pub fn recv_blocking_with_cancel(&mut self, cancel: &AtomicBool) -> Option<GeofenceEvent> {
        loop {
            let current_generation = *NativePlatform::read(&*self.generation);
            let event = self.receiver.recv_blocking_with_cancel(cancel)?;
            if event.generation >= current_generation {
                return Some(event);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelDef {
    pub name: String,
    pub def: GeofenceDef,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookDef {
    pub name: String,
    pub endpoint: String,
    pub def: GeofenceDef,
}

#[derive(Debug, Error)]
pub enum GeofenceError {
    #[error(transparent)]
    Geo(#[from] GeoError),
    #[error(transparent)]
    Index(#[from] IndexError),
}

pub type GeofenceResult<T> = Result<T, GeofenceError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MutationCommand {
    Set,
    Del,
    Drop,
    Fset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DetectType {
    Inside,
    Outside,
    Enter,
    Exit,
    Cross,
    Roam,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GeofenceQuery {
    Nearby {
        lat: f64,
        lon: f64,
        meters: f64,
        options: SearchOptions,
    },
    Within {
        area: Area,
        options: SearchOptions,
    },
    Intersects {
        area: Area,
        options: SearchOptions,
    },
    Roam {
        target_collection: String,
        target_pattern: String,
        meters: f64,
        options: SearchOptions,
        nodwell: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeofenceDef {
    pub collection: String,
    pub query: GeofenceQuery,
    pub detect: Vec<DetectType>,
    pub commands: Vec<MutationCommand>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookInfo {
    pub name: String,
    pub endpoint: String,
    pub collection: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoamingInfo {
    pub collection: String,
    pub id: String,
    pub meters: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeofenceEvent {
    pub command: MutationCommand,
    pub detect: DetectType,
    pub collection: String,
    pub id: String,
    pub object: GeoType,
    pub fields: latlng_geo::FieldMap,
    pub timestamp_ns: u64,
    /// Stable opaque event identifier for external consumers and idempotency.
    pub event_id: Option<String>,
    /// Stable opaque webhook delivery job identifier.
    pub job_id: Option<String>,
    pub hook: Option<String>,
    pub group: Option<String>,
    pub nearby: Option<RoamingInfo>,
    #[serde(skip)]
    pub generation: u64,
}

#[derive(Debug, Clone)]
pub struct MutationEvent {
    pub command: MutationCommand,
    pub collection: String,
    pub id: String,
    pub before: Option<Object>,
    pub after: Option<Object>,
    pub timestamp_ns: u64,
}

pub struct GeofenceRegistry<P: Platform> {
    subscriber_queue_capacity: usize,
    channels: HashMap<String, StoredGeofence>,
    channels_by_collection: HashMap<String, Vec<String>>,
    channels_by_target_collection: HashMap<String, Vec<String>>,
    hooks: HashMap<String, StoredHook>,
    hooks_by_collection: HashMap<String, Vec<String>>,
    hooks_by_target_collection: HashMap<String, Vec<String>>,
    cross_collection_roaming_collections: HashSet<String>,
    subscribers: Vec<Subscriber<P>>,
    generation: P::Shared<P::RwLock<u64>>,
}

pub struct PreparedMutation {
    channel_states: Vec<(String, StoredStateUpdate)>,
    hook_states: Vec<(String, StoredStateUpdate)>,
    events: Vec<GeofenceEvent>,
}

impl PreparedMutation {
    pub fn events(&self) -> &[GeofenceEvent] {
        &self.events
    }
}

#[derive(Debug, Clone)]
struct StoredGeofence {
    def: GeofenceDef,
    state: StoredState,
}

#[derive(Debug, Clone)]
struct StoredHook {
    endpoint: String,
    def: GeofenceDef,
    state: StoredState,
}

#[derive(Debug, Clone, Default)]
struct StoredState {
    objects: HashMap<String, bool>,
    pairs: HashMap<(String, String), bool>,
}

#[derive(Debug, Clone)]
enum StoredStateUpdate {
    None,
    Object { id: String, inside: Option<bool> },
    Replace(StoredState),
}

impl StoredStateUpdate {
    fn apply(self, state: &mut StoredState) {
        match self {
            Self::None => {}
            Self::Object { id, inside } => {
                if let Some(inside) = inside {
                    state.objects.insert(id, inside);
                } else {
                    state.objects.remove(&id);
                }
            }
            Self::Replace(next) => *state = next,
        }
    }
}

struct Subscriber<P: Platform> {
    exact: Vec<String>,
    patterns: Vec<String>,
    sender: P::Sender<GeofenceEvent>,
}

impl<P: Platform> Default for GeofenceRegistry<P> {
    fn default() -> Self {
        Self {
            subscriber_queue_capacity: DEFAULT_SUBSCRIBER_QUEUE_CAPACITY,
            channels: HashMap::new(),
            channels_by_collection: HashMap::new(),
            channels_by_target_collection: HashMap::new(),
            hooks: HashMap::new(),
            hooks_by_collection: HashMap::new(),
            hooks_by_target_collection: HashMap::new(),
            cross_collection_roaming_collections: HashSet::new(),
            subscribers: Vec::new(),
            generation: P::shared(P::new_rwlock(0_u64)),
        }
    }
}

impl<P: Platform> GeofenceRegistry<P> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_subscriber_queue_capacity(subscriber_queue_capacity: usize) -> Self {
        Self {
            subscriber_queue_capacity: subscriber_queue_capacity.max(1),
            ..Self::default()
        }
    }

    pub fn set_channel(&mut self, name: impl Into<String>, def: GeofenceDef) {
        let name = name.into();
        if let Some(previous) = self.channels.remove(&name) {
            self.remove_channel_indexes(&name, &previous.def);
        }
        self.insert_channel_indexes(&name, &def);
        self.channels.insert(
            name.clone(),
            StoredGeofence {
                def,
                state: StoredState::default(),
            },
        );
        self.refresh_cross_collection_roaming_collections();
    }

    pub fn del_channel(&mut self, name: &str) -> bool {
        let removed = self.channels.remove(name);
        if let Some(stored) = removed {
            self.remove_channel_indexes(name, &stored.def);
            self.refresh_cross_collection_roaming_collections();
            true
        } else {
            false
        }
    }

    pub fn pdel_channel(&mut self, pattern: &str) -> u64 {
        let names = self
            .channels
            .keys()
            .filter(|name| glob_match(pattern, name))
            .cloned()
            .collect::<Vec<_>>();
        let count = names.len() as u64;
        for name in names {
            let _ = self.del_channel(&name);
        }
        count
    }

    pub fn channels(&self, pattern: &str) -> Vec<String> {
        let mut names = self
            .channels
            .keys()
            .filter(|name| glob_match(pattern, name))
            .cloned()
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    pub fn channel_defs(&self) -> Vec<ChannelDef> {
        let mut channels = self
            .channels
            .iter()
            .map(|(name, stored)| ChannelDef {
                name: name.clone(),
                def: stored.def.clone(),
            })
            .collect::<Vec<_>>();
        channels.sort_by(|left, right| left.name.cmp(&right.name));
        channels
    }

    pub fn channel_def(&self, name: &str) -> Option<ChannelDef> {
        self.channels.get(name).map(|stored| ChannelDef {
            name: name.to_owned(),
            def: stored.def.clone(),
        })
    }

    pub fn set_hook(
        &mut self,
        name: impl Into<String>,
        endpoint: impl Into<String>,
        def: GeofenceDef,
    ) {
        let name = name.into();
        if let Some(previous) = self.hooks.remove(&name) {
            self.remove_hook_indexes(&name, &previous.def);
        }
        self.insert_hook_indexes(&name, &def);
        self.hooks.insert(
            name.clone(),
            StoredHook {
                endpoint: endpoint.into(),
                def,
                state: StoredState::default(),
            },
        );
        self.refresh_cross_collection_roaming_collections();
    }

    pub fn del_hook(&mut self, name: &str) -> bool {
        let removed = self.hooks.remove(name);
        if let Some(stored) = removed {
            self.remove_hook_indexes(name, &stored.def);
            self.refresh_cross_collection_roaming_collections();
            true
        } else {
            false
        }
    }

    pub fn pdel_hook(&mut self, pattern: &str) -> u64 {
        let names = self
            .hooks
            .keys()
            .filter(|name| glob_match(pattern, name))
            .cloned()
            .collect::<Vec<_>>();
        let count = names.len() as u64;
        for name in names {
            let _ = self.del_hook(&name);
        }
        count
    }

    pub fn hooks(&self, pattern: &str) -> Vec<HookInfo> {
        let mut hooks = self
            .hooks
            .iter()
            .filter(|(name, _)| glob_match(pattern, name))
            .map(|(name, hook)| HookInfo {
                name: name.clone(),
                endpoint: hook.endpoint.clone(),
                collection: hook.def.collection.clone(),
            })
            .collect::<Vec<_>>();
        hooks.sort_by(|left, right| left.name.cmp(&right.name));
        hooks
    }

    pub fn hook_defs(&self) -> Vec<HookDef> {
        let mut hooks = self
            .hooks
            .iter()
            .map(|(name, stored)| HookDef {
                name: name.clone(),
                endpoint: stored.endpoint.clone(),
                def: stored.def.clone(),
            })
            .collect::<Vec<_>>();
        hooks.sort_by(|left, right| left.name.cmp(&right.name));
        hooks
    }

    pub fn hook_def(&self, name: &str) -> Option<HookDef> {
        self.hooks.get(name).map(|stored| HookDef {
            name: name.to_owned(),
            endpoint: stored.endpoint.clone(),
            def: stored.def.clone(),
        })
    }

    pub fn subscribe(&mut self, channels: &[&str]) -> GeofenceEventReceiver<P> {
        let (sender, receiver) = P::channel(self.subscriber_queue_capacity);
        self.subscribers.push(Subscriber {
            exact: channels.iter().map(|value| (*value).to_owned()).collect(),
            patterns: Vec::new(),
            sender,
        });
        GeofenceEventReceiver::new(receiver, self.generation.clone())
    }

    pub fn psubscribe(&mut self, patterns: &[&str]) -> GeofenceEventReceiver<P> {
        let (sender, receiver) = P::channel(self.subscriber_queue_capacity);
        self.subscribers.push(Subscriber {
            exact: Vec::new(),
            patterns: patterns.iter().map(|value| (*value).to_owned()).collect(),
            sender,
        });
        GeofenceEventReceiver::new(receiver, self.generation.clone())
    }

    pub fn clear_all(&mut self) {
        self.channels.clear();
        self.channels_by_collection.clear();
        self.channels_by_target_collection.clear();
        self.hooks.clear();
        self.hooks_by_collection.clear();
        self.hooks_by_target_collection.clear();
        self.cross_collection_roaming_collections.clear();
        let mut generation = P::write(&*self.generation);
        *generation = generation.saturating_add(1);
    }

    pub fn requires_exclusive_roam_path(&self, collection: &str) -> bool {
        self.cross_collection_roaming_collections
            .contains(collection)
    }

    pub fn has_relevant_side_effects(&self, collection: &str) -> bool {
        has_indexed_names(&self.channels_by_collection, collection)
            || has_indexed_names(&self.channels_by_target_collection, collection)
            || has_indexed_names(&self.hooks_by_collection, collection)
            || has_indexed_names(&self.hooks_by_target_collection, collection)
    }

    pub fn evaluate_mutation<F>(
        &mut self,
        event: &MutationEvent,
        lookup: &F,
    ) -> GeofenceResult<Vec<GeofenceEvent>>
    where
        F: Fn(&str) -> Vec<Object>,
    {
        let prepared = self.prepare_mutation(event, lookup)?;
        let produced = prepared.events.clone();
        self.apply_prepared_mutation(prepared);
        Ok(produced)
    }

    pub fn prepare_mutation<F>(
        &self,
        event: &MutationEvent,
        lookup: &F,
    ) -> GeofenceResult<PreparedMutation>
    where
        F: Fn(&str) -> Vec<Object>,
    {
        let channel_names = self.relevant_channel_names(&event.collection);
        let hook_names = self.relevant_hook_names(&event.collection);
        let mut channel_states = Vec::with_capacity(channel_names.len());
        let mut hook_states = Vec::with_capacity(hook_names.len());
        let mut events = Vec::new();

        for name in channel_names {
            let Some(stored) = self.channels.get(&name) else {
                continue;
            };
            let (state_update, produced) = evaluate_stored_fence(
                &stored.def,
                &stored.state,
                event,
                Some(&name),
                None,
                lookup,
            )?;
            events.extend(produced);
            channel_states.push((name, state_update));
        }

        for name in hook_names {
            let Some(stored) = self.hooks.get(&name) else {
                continue;
            };
            let (state_update, produced) = evaluate_stored_fence(
                &stored.def,
                &stored.state,
                event,
                Some(&name),
                Some(&name),
                lookup,
            )?;
            events.extend(produced);
            hook_states.push((name, state_update));
        }

        Ok(PreparedMutation {
            channel_states,
            hook_states,
            events,
        })
    }

    pub fn apply_prepared_mutation(&mut self, prepared: PreparedMutation) {
        for (name, state_update) in prepared.channel_states {
            if let Some(stored) = self.channels.get_mut(&name) {
                state_update.apply(&mut stored.state);
            }
        }
        for (name, state_update) in prepared.hook_states {
            if let Some(stored) = self.hooks.get_mut(&name) {
                state_update.apply(&mut stored.state);
            }
        }
        let generation = *P::read(&*self.generation);
        for mut event in prepared.events {
            event.generation = generation;
            self.publish(event);
        }
    }

    fn publish(&mut self, event: GeofenceEvent) {
        for subscriber in &self.subscribers {
            let Some(group) = event.group.as_deref() else {
                continue;
            };
            let exact_match = subscriber.exact.iter().any(|candidate| candidate == group);
            let pattern_match = subscriber
                .patterns
                .iter()
                .any(|pattern| glob_match(pattern, group));
            if exact_match || pattern_match {
                let _ = subscriber.sender.send(event.clone());
            }
        }
    }

    fn relevant_channel_names(&self, collection: &str) -> Vec<String> {
        let mut names = Vec::new();
        let mut seen = HashSet::new();
        collect_indexed_names(
            self.channels_by_collection.get(collection),
            &mut seen,
            &mut names,
        );
        collect_indexed_names(
            self.channels_by_target_collection.get(collection),
            &mut seen,
            &mut names,
        );
        names
    }

    fn relevant_hook_names(&self, collection: &str) -> Vec<String> {
        let mut names = Vec::new();
        let mut seen = HashSet::new();
        collect_indexed_names(
            self.hooks_by_collection.get(collection),
            &mut seen,
            &mut names,
        );
        collect_indexed_names(
            self.hooks_by_target_collection.get(collection),
            &mut seen,
            &mut names,
        );
        names
    }

    fn insert_channel_indexes(&mut self, name: &str, def: &GeofenceDef) {
        push_index_name(&mut self.channels_by_collection, &def.collection, name);
        if let GeofenceQuery::Roam {
            target_collection, ..
        } = &def.query
        {
            push_index_name(
                &mut self.channels_by_target_collection,
                target_collection,
                name,
            );
        }
    }

    fn remove_channel_indexes(&mut self, name: &str, def: &GeofenceDef) {
        remove_index_name(&mut self.channels_by_collection, &def.collection, name);
        if let GeofenceQuery::Roam {
            target_collection, ..
        } = &def.query
        {
            remove_index_name(
                &mut self.channels_by_target_collection,
                target_collection,
                name,
            );
        }
    }

    fn insert_hook_indexes(&mut self, name: &str, def: &GeofenceDef) {
        push_index_name(&mut self.hooks_by_collection, &def.collection, name);
        if let GeofenceQuery::Roam {
            target_collection, ..
        } = &def.query
        {
            push_index_name(
                &mut self.hooks_by_target_collection,
                target_collection,
                name,
            );
        }
    }

    fn remove_hook_indexes(&mut self, name: &str, def: &GeofenceDef) {
        remove_index_name(&mut self.hooks_by_collection, &def.collection, name);
        if let GeofenceQuery::Roam {
            target_collection, ..
        } = &def.query
        {
            remove_index_name(
                &mut self.hooks_by_target_collection,
                target_collection,
                name,
            );
        }
    }

    fn refresh_cross_collection_roaming_collections(&mut self) {
        self.cross_collection_roaming_collections.clear();
        for def in self
            .channels
            .values()
            .map(|stored| &stored.def)
            .chain(self.hooks.values().map(|stored| &stored.def))
        {
            if let GeofenceQuery::Roam {
                target_collection, ..
            } = &def.query
                && target_collection != &def.collection
            {
                self.cross_collection_roaming_collections
                    .insert(def.collection.clone());
                self.cross_collection_roaming_collections
                    .insert(target_collection.clone());
            }
        }
    }
}

fn collect_indexed_names(
    source: Option<&Vec<String>>,
    seen: &mut HashSet<String>,
    output: &mut Vec<String>,
) {
    if let Some(names) = source {
        for name in names {
            if seen.insert(name.clone()) {
                output.push(name.clone());
            }
        }
    }
}

fn push_index_name(index: &mut HashMap<String, Vec<String>>, collection: &str, name: &str) {
    let names = index.entry(collection.to_owned()).or_default();
    if !names.iter().any(|candidate| candidate == name) {
        names.push(name.to_owned());
    }
}

fn has_indexed_names(index: &HashMap<String, Vec<String>>, collection: &str) -> bool {
    index.get(collection).is_some_and(|names| !names.is_empty())
}

fn remove_index_name(index: &mut HashMap<String, Vec<String>>, collection: &str, name: &str) {
    let remove_entry = if let Some(names) = index.get_mut(collection) {
        names.retain(|candidate| candidate != name);
        names.is_empty()
    } else {
        false
    };
    if remove_entry {
        index.remove(collection);
    }
}

fn evaluate_stored_fence(
    def: &GeofenceDef,
    state: &StoredState,
    event: &MutationEvent,
    group: Option<&str>,
    hook: Option<&str>,
    lookup: &impl Fn(&str) -> Vec<Object>,
) -> GeofenceResult<(StoredStateUpdate, Vec<GeofenceEvent>)> {
    match &def.query {
        GeofenceQuery::Roam {
            target_collection,
            target_pattern,
            meters,
            options,
            nodwell,
        } => {
            let mut next_state = state.clone();
            let events = evaluate_roaming_fence(
                def,
                &mut next_state,
                event,
                group,
                hook,
                target_collection,
                target_pattern,
                *meters,
                options,
                *nodwell,
                lookup,
            )?;
            Ok((StoredStateUpdate::Replace(next_state), events))
        }
        GeofenceQuery::Nearby { .. }
        | GeofenceQuery::Within { .. }
        | GeofenceQuery::Intersects { .. } => evaluate_static_fence(def, state, event, group, hook),
    }
}

fn evaluate_static_fence(
    def: &GeofenceDef,
    state: &StoredState,
    event: &MutationEvent,
    group: Option<&str>,
    hook: Option<&str>,
) -> GeofenceResult<(StoredStateUpdate, Vec<GeofenceEvent>)> {
    if def.collection != event.collection {
        return Ok((StoredStateUpdate::None, Vec::new()));
    }

    let command_allowed = def.commands.is_empty() || def.commands.contains(&event.command);
    let previous_inside = *state.objects.get(&event.id).unwrap_or(&false);
    let current_inside = event
        .after
        .as_ref()
        .map(|object| object_matches_query(object, def))
        .transpose()?
        .unwrap_or(false);

    let detect = if !previous_inside && current_inside {
        DetectType::Enter
    } else if previous_inside && current_inside {
        DetectType::Inside
    } else if previous_inside && !current_inside {
        DetectType::Exit
    } else if crossed_boundary(event, def)? {
        DetectType::Cross
    } else {
        DetectType::Outside
    };

    let state_update = if event.after.is_some() {
        StoredStateUpdate::Object {
            id: event.id.clone(),
            inside: Some(current_inside),
        }
    } else {
        StoredStateUpdate::Object {
            id: event.id.clone(),
            inside: None,
        }
    };

    if !def.detect.is_empty() && !def.detect.contains(&detect) {
        return Ok((state_update, Vec::new()));
    }

    if !command_allowed {
        return Ok((state_update, Vec::new()));
    }

    let object = event
        .after
        .as_ref()
        .or(event.before.as_ref())
        .map(|object| object.geo.clone())
        .unwrap_or_else(|| GeoType::String(event.id.clone()));
    let fields = event
        .after
        .as_ref()
        .or(event.before.as_ref())
        .map(|object| object.fields.clone())
        .unwrap_or_default();

    Ok((
        state_update,
        vec![GeofenceEvent {
            command: event.command,
            detect,
            collection: event.collection.clone(),
            id: event.id.clone(),
            object,
            fields,
            timestamp_ns: event.timestamp_ns,
            event_id: None,
            job_id: None,
            hook: hook.map(str::to_owned),
            group: group.map(str::to_owned),
            nearby: None,
            generation: 0,
        }],
    ))
}

#[allow(clippy::too_many_arguments)]
fn evaluate_roaming_fence<F>(
    def: &GeofenceDef,
    state: &mut StoredState,
    event: &MutationEvent,
    group: Option<&str>,
    hook: Option<&str>,
    target_collection: &str,
    target_pattern: &str,
    meters: f64,
    options: &SearchOptions,
    nodwell: bool,
    lookup: &F,
) -> GeofenceResult<Vec<GeofenceEvent>>
where
    F: Fn(&str) -> Vec<Object>,
{
    if event.collection != def.collection && event.collection != target_collection {
        return Ok(Vec::new());
    }

    let command_allowed = def.commands.is_empty() || def.commands.contains(&event.command);
    let detect_allowed = def.detect.is_empty() || def.detect.contains(&DetectType::Roam);
    let mut events = Vec::new();

    if event.collection == def.collection {
        if event.after.is_none() {
            clear_source_pairs(state, &event.id);
            return Ok(Vec::new());
        }
        let Some(source) = event.after.as_ref() else {
            return Ok(Vec::new());
        };

        reconcile_source_pairs(
            state,
            source,
            lookup(target_collection),
            target_collection,
            target_pattern,
            meters,
            options,
            nodwell,
            event,
            def.collection.as_str(),
            group,
            hook,
            command_allowed && detect_allowed,
            &mut events,
        )?;
        return Ok(events);
    }

    let source_objects = lookup(&def.collection);
    let targets = event
        .after
        .iter()
        .filter(|object| glob_match(target_pattern, &object.id))
        .cloned()
        .collect::<Vec<_>>();

    for source in source_objects {
        reconcile_target_pair(
            state,
            &source,
            &targets,
            event
                .before
                .as_ref()
                .or(event.after.as_ref())
                .map(|object| object.id.as_str()),
            meters,
            options,
            nodwell,
            event,
            def.collection.as_str(),
            target_collection,
            group,
            hook,
            command_allowed && detect_allowed,
            &mut events,
        )?;
    }

    Ok(events)
}

#[allow(clippy::too_many_arguments)]
fn reconcile_source_pairs(
    state: &mut StoredState,
    source: &Object,
    targets: Vec<Object>,
    target_collection: &str,
    target_pattern: &str,
    meters: f64,
    options: &SearchOptions,
    nodwell: bool,
    event: &MutationEvent,
    source_collection: &str,
    group: Option<&str>,
    hook: Option<&str>,
    emit: bool,
    events: &mut Vec<GeofenceEvent>,
) -> GeofenceResult<()> {
    let source_allowed = matches_candidate(source, options)?;
    let mut current_matches = HashMap::new();

    if source_allowed {
        for target in targets {
            if !glob_match(target_pattern, &target.id) || target.id == source.id {
                continue;
            }
            if let Some(distance) = roam_distance_meters(source, &target, meters)? {
                current_matches.insert(target.id.clone(), (target, distance));
            }
        }
    }

    let previous_targets = state
        .pairs
        .keys()
        .filter_map(|(source_id, target_id)| {
            if source_id == &source.id {
                Some(target_id.clone())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    for target_id in previous_targets {
        if !current_matches.contains_key(&target_id) {
            state.pairs.remove(&(source.id.clone(), target_id));
        }
    }

    for (target_id, (_target, distance)) in current_matches {
        let key = (source.id.clone(), target_id.clone());
        let was_near = state.pairs.insert(key, true).unwrap_or(false);
        if !emit || (was_near && nodwell) {
            continue;
        }
        events.push(roam_event(
            event,
            source_collection,
            source,
            target_collection.to_owned(),
            &target_id,
            distance,
            group,
            hook,
        ));
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn reconcile_target_pair(
    state: &mut StoredState,
    source: &Object,
    targets: &[Object],
    target_id: Option<&str>,
    meters: f64,
    options: &SearchOptions,
    nodwell: bool,
    event: &MutationEvent,
    source_collection: &str,
    target_collection: &str,
    group: Option<&str>,
    hook: Option<&str>,
    emit: bool,
    events: &mut Vec<GeofenceEvent>,
) -> GeofenceResult<()> {
    let Some(target_id) = target_id else {
        return Ok(());
    };
    let key = (source.id.clone(), target_id.to_owned());

    if !matches_candidate(source, options)? {
        state.pairs.remove(&key);
        return Ok(());
    }

    let Some((target, distance)) = targets.iter().find_map(|target| {
        if target.id == source.id {
            return None;
        }
        roam_distance_meters(source, target, meters)
            .ok()
            .flatten()
            .map(|distance| (target, distance))
    }) else {
        state.pairs.remove(&key);
        return Ok(());
    };

    let was_near = state.pairs.insert(key, true).unwrap_or(false);
    if emit && (!was_near || !nodwell) {
        events.push(roam_event(
            event,
            source_collection,
            source,
            target_collection.to_owned(),
            &target.id,
            distance,
            group,
            hook,
        ));
    }

    Ok(())
}

fn clear_source_pairs(state: &mut StoredState, source_id: &str) {
    let keys = state
        .pairs
        .keys()
        .filter(|(candidate, _)| candidate == source_id)
        .cloned()
        .collect::<Vec<_>>();
    for key in keys {
        state.pairs.remove(&key);
    }
}

fn roam_distance_meters(
    source: &Object,
    target: &Object,
    meters: f64,
) -> GeofenceResult<Option<f64>> {
    let Some((source_lat, source_lon)) = source.geo.point_coordinates() else {
        return Ok(None);
    };
    let Some((target_lat, target_lon)) = target.geo.point_coordinates() else {
        return Ok(None);
    };
    let distance = haversine_distance_meters(source_lat, source_lon, target_lat, target_lon);
    if distance <= meters {
        Ok(Some(distance))
    } else {
        Ok(None)
    }
}

#[allow(clippy::too_many_arguments)]
fn roam_event(
    event: &MutationEvent,
    source_collection: &str,
    source: &Object,
    target_collection: String,
    target_id: &str,
    meters: f64,
    group: Option<&str>,
    hook: Option<&str>,
) -> GeofenceEvent {
    GeofenceEvent {
        command: event.command,
        detect: DetectType::Roam,
        collection: source_collection.to_owned(),
        id: source.id.clone(),
        object: source.geo.clone(),
        fields: source.fields.clone(),
        timestamp_ns: event.timestamp_ns,
        event_id: None,
        job_id: None,
        hook: hook.map(str::to_owned),
        group: group.map(str::to_owned),
        nearby: Some(RoamingInfo {
            collection: target_collection,
            id: target_id.to_owned(),
            meters,
        }),
        generation: 0,
    }
}

fn object_matches_query(object: &Object, def: &GeofenceDef) -> GeofenceResult<bool> {
    match &def.query {
        GeofenceQuery::Nearby {
            lat,
            lon,
            meters,
            options,
        } => {
            let within_distance = object
                .geo
                .point_coordinates()
                .map(|(point_lat, point_lon)| {
                    haversine_distance_meters(*lat, *lon, point_lat, point_lon) <= *meters
                })
                .unwrap_or_else(|| {
                    object
                        .envelope()
                        .ok()
                        .flatten()
                        .map(|bounds| {
                            let (center_lat, center_lon) = bounds.center();
                            haversine_distance_meters(*lat, *lon, center_lat, center_lon) <= *meters
                        })
                        .unwrap_or(false)
                });
            if !within_distance {
                return Ok(false);
            }
            matches_candidate(object, options).map_err(Into::into)
        }
        GeofenceQuery::Within { area, options } => {
            if !area.contains_geo(&object.geo)? {
                return Ok(false);
            }
            matches_candidate(object, options).map_err(Into::into)
        }
        GeofenceQuery::Intersects { area, options } => {
            if !area.intersects_geo(&object.geo)? {
                return Ok(false);
            }
            matches_candidate(object, options).map_err(Into::into)
        }
        GeofenceQuery::Roam { options, .. } => {
            matches_candidate(object, options).map_err(Into::into)
        }
    }
}

fn crossed_boundary(event: &MutationEvent, def: &GeofenceDef) -> GeofenceResult<bool> {
    let Some(before) = event.before.as_ref() else {
        return Ok(false);
    };
    let Some(after) = event.after.as_ref() else {
        return Ok(false);
    };
    let Some((before_lat, before_lon)) = before.geo.point_coordinates() else {
        return Ok(false);
    };
    let Some((after_lat, after_lon)) = after.geo.point_coordinates() else {
        return Ok(false);
    };

    let line = geo_types::Geometry::LineString(LineString::from(vec![
        (before_lon, before_lat),
        (after_lon, after_lat),
    ]));
    match &def.query {
        GeofenceQuery::Nearby {
            lat, lon, meters, ..
        } => {
            let area = Area::Circle {
                lat: *lat,
                lon: *lon,
                meters: *meters,
            };
            Ok(area.to_geometry()?.intersects(&line))
        }
        GeofenceQuery::Within { area, .. } | GeofenceQuery::Intersects { area, .. } => {
            Ok(area.to_geometry()?.intersects(&line))
        }
        GeofenceQuery::Roam { .. } => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use latlng_geo::{FieldMap, GeoType, Object};
    use latlng_index::SearchOptions;
    use latlng_platform::NativePlatform;

    use super::{
        DetectType, GeofenceDef, GeofenceQuery, GeofenceRegistry, MutationCommand, MutationEvent,
    };

    #[test]
    fn enter_event_is_emitted_for_matching_point() {
        let mut registry = GeofenceRegistry::<NativePlatform>::new();
        registry.set_channel(
            "fleet",
            GeofenceDef {
                collection: "fleet".to_owned(),
                query: GeofenceQuery::Nearby {
                    lat: 52.52,
                    lon: 13.405,
                    meters: 100.0,
                    options: SearchOptions::default(),
                },
                detect: vec![DetectType::Enter],
                commands: vec![MutationCommand::Set],
            },
        );
        let mut receiver = registry.subscribe(&["fleet"]);
        let events = registry
            .evaluate_mutation(
                &MutationEvent {
                    command: MutationCommand::Set,
                    collection: "fleet".to_owned(),
                    id: "truck-1".to_owned(),
                    before: None,
                    after: Some(Object {
                        id: "truck-1".to_owned(),
                        geo: GeoType::point(52.52, 13.405),
                        fields: FieldMap::new(),
                        expires_at: None,
                    }),
                    timestamp_ns: 1,
                },
                &|_| Vec::new(),
            )
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(receiver.try_recv().unwrap().detect, DetectType::Enter);
    }

    #[test]
    fn static_fences_cover_state_transitions() {
        let mut registry = GeofenceRegistry::<NativePlatform>::new();
        let def = GeofenceDef {
            collection: "fleet".to_owned(),
            query: GeofenceQuery::Nearby {
                lat: 52.52,
                lon: 13.405,
                meters: 100.0,
                options: SearchOptions::default(),
            },
            detect: Vec::new(),
            commands: vec![MutationCommand::Set, MutationCommand::Del],
        };
        registry.set_channel("fleet", def);

        let outside = registry
            .evaluate_mutation(
                &mutation("fleet", "truck-1", None, Some(point(0.0, 0.0)), 1),
                &|_| Vec::new(),
            )
            .unwrap();
        assert_eq!(outside[0].detect, DetectType::Outside);

        let enter = registry
            .evaluate_mutation(
                &mutation(
                    "fleet",
                    "truck-1",
                    Some(point(0.0, 0.0)),
                    Some(point(52.52, 13.405)),
                    2,
                ),
                &|_| Vec::new(),
            )
            .unwrap();
        assert_eq!(enter[0].detect, DetectType::Enter);

        let inside = registry
            .evaluate_mutation(
                &mutation(
                    "fleet",
                    "truck-1",
                    Some(point(52.52, 13.405)),
                    Some(point(52.5204, 13.4054)),
                    3,
                ),
                &|_| Vec::new(),
            )
            .unwrap();
        assert_eq!(inside[0].detect, DetectType::Inside);

        let exit = registry
            .evaluate_mutation(
                &mutation(
                    "fleet",
                    "truck-1",
                    Some(point(52.5204, 13.4054)),
                    Some(point(0.0, 0.0)),
                    4,
                ),
                &|_| Vec::new(),
            )
            .unwrap();
        assert_eq!(exit[0].detect, DetectType::Exit);

        let cross = registry
            .evaluate_mutation(
                &mutation(
                    "fleet",
                    "truck-2",
                    Some(point(52.519, 13.403)),
                    Some(point(52.521, 13.407)),
                    5,
                ),
                &|_| Vec::new(),
            )
            .unwrap();
        assert_eq!(cross[0].detect, DetectType::Cross);
    }

    #[test]
    fn roaming_fence_emits_and_nodwell_suppresses_repeat_pairs() {
        let mut registry = GeofenceRegistry::<NativePlatform>::new();
        registry.set_channel(
            "drivers",
            GeofenceDef {
                collection: "drivers".to_owned(),
                query: GeofenceQuery::Roam {
                    target_collection: "riders".to_owned(),
                    target_pattern: "rider-*".to_owned(),
                    meters: 100.0,
                    options: SearchOptions::default(),
                    nodwell: true,
                },
                detect: vec![DetectType::Roam],
                commands: vec![MutationCommand::Set],
            },
        );

        let lookup = |collection: &str| match collection {
            "drivers" => vec![object("driver-1", 52.52, 13.405)],
            "riders" => vec![object("rider-1", 52.5202, 13.4052)],
            _ => Vec::new(),
        };

        let first = registry
            .evaluate_mutation(
                &mutation("drivers", "driver-1", None, Some(point(52.52, 13.405)), 1),
                &lookup,
            )
            .unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].detect, DetectType::Roam);
        assert_eq!(
            first[0].nearby.as_ref().map(|info| info.id.as_str()),
            Some("rider-1")
        );

        let second = registry
            .evaluate_mutation(
                &mutation(
                    "drivers",
                    "driver-1",
                    Some(point(52.52, 13.405)),
                    Some(point(52.5201, 13.4051)),
                    2,
                ),
                &lookup,
            )
            .unwrap();
        assert!(second.is_empty());
    }

    #[test]
    fn subscriber_queue_capacity_is_respected() {
        let mut registry = GeofenceRegistry::<NativePlatform>::with_subscriber_queue_capacity(1);
        registry.set_channel(
            "fleet",
            GeofenceDef {
                collection: "fleet".to_owned(),
                query: GeofenceQuery::Nearby {
                    lat: 52.52,
                    lon: 13.405,
                    meters: 100.0,
                    options: SearchOptions::default(),
                },
                detect: vec![DetectType::Enter, DetectType::Inside],
                commands: vec![MutationCommand::Set],
            },
        );
        let mut receiver = registry.subscribe(&["fleet"]);

        let _ = registry.evaluate_mutation(
            &mutation("fleet", "truck-1", None, Some(point(52.52, 13.405)), 1),
            &|_| Vec::new(),
        );
        let _ = registry.evaluate_mutation(
            &mutation(
                "fleet",
                "truck-1",
                Some(point(52.52, 13.405)),
                Some(point(52.5202, 13.4052)),
                2,
            ),
            &|_| Vec::new(),
        );

        assert_eq!(receiver.try_recv().unwrap().detect, DetectType::Inside);
        assert!(receiver.try_recv().is_none());
    }

    #[test]
    fn clear_all_preserves_subscribers_but_drops_stale_events() {
        let mut registry = GeofenceRegistry::<NativePlatform>::new();
        let def = GeofenceDef {
            collection: "fleet".to_owned(),
            query: GeofenceQuery::Nearby {
                lat: 52.52,
                lon: 13.405,
                meters: 100.0,
                options: SearchOptions::default(),
            },
            detect: vec![DetectType::Enter],
            commands: vec![MutationCommand::Set],
        };
        registry.set_channel("fleet", def.clone());
        let mut receiver = registry.subscribe(&["fleet"]);

        registry
            .evaluate_mutation(
                &mutation("fleet", "truck-1", None, Some(point(52.52, 13.405)), 1),
                &|_| Vec::new(),
            )
            .unwrap();

        registry.clear_all();
        registry.set_channel("fleet", def);
        registry
            .evaluate_mutation(
                &mutation("fleet", "truck-1", None, Some(point(52.52, 13.405)), 2),
                &|_| Vec::new(),
            )
            .unwrap();

        let event = receiver.try_recv().unwrap();
        assert_eq!(event.detect, DetectType::Enter);
        assert_eq!(event.timestamp_ns, 2);
        assert!(receiver.try_recv().is_none());
    }

    #[test]
    fn cross_collection_roam_path_tracking_updates_with_definitions() {
        let mut registry = GeofenceRegistry::<NativePlatform>::new();
        assert!(!registry.requires_exclusive_roam_path("drivers"));
        assert!(!registry.requires_exclusive_roam_path("riders"));

        registry.set_channel(
            "drivers",
            GeofenceDef {
                collection: "drivers".to_owned(),
                query: GeofenceQuery::Roam {
                    target_collection: "riders".to_owned(),
                    target_pattern: "rider-*".to_owned(),
                    meters: 100.0,
                    options: SearchOptions::default(),
                    nodwell: false,
                },
                detect: vec![DetectType::Roam],
                commands: vec![MutationCommand::Set],
            },
        );

        assert!(registry.requires_exclusive_roam_path("drivers"));
        assert!(registry.requires_exclusive_roam_path("riders"));

        registry.del_channel("drivers");
        assert!(!registry.requires_exclusive_roam_path("drivers"));
        assert!(!registry.requires_exclusive_roam_path("riders"));
    }

    #[test]
    fn unrelated_static_fences_do_not_emit_events() {
        let mut registry = GeofenceRegistry::<NativePlatform>::new();
        registry.set_channel(
            "fleet",
            GeofenceDef {
                collection: "fleet".to_owned(),
                query: GeofenceQuery::Nearby {
                    lat: 52.52,
                    lon: 13.405,
                    meters: 100.0,
                    options: SearchOptions::default(),
                },
                detect: vec![DetectType::Enter],
                commands: vec![MutationCommand::Set],
            },
        );

        let events = registry
            .evaluate_mutation(
                &mutation("orders", "order-1", None, Some(point(52.52, 13.405)), 1),
                &|_| Vec::new(),
            )
            .unwrap();

        assert!(events.is_empty());
    }

    #[test]
    fn unrelated_hooks_do_not_enqueue_and_related_hook_preserves_identity() {
        let mut registry = GeofenceRegistry::<NativePlatform>::new();
        registry.set_hook(
            "orders-hook",
            "https://example.invalid/orders",
            GeofenceDef {
                collection: "orders".to_owned(),
                query: GeofenceQuery::Nearby {
                    lat: 52.52,
                    lon: 13.405,
                    meters: 100.0,
                    options: SearchOptions::default(),
                },
                detect: vec![DetectType::Enter],
                commands: vec![MutationCommand::Set],
            },
        );
        registry.set_hook(
            "fleet-hook",
            "https://example.invalid/fleet",
            GeofenceDef {
                collection: "fleet".to_owned(),
                query: GeofenceQuery::Nearby {
                    lat: 52.52,
                    lon: 13.405,
                    meters: 100.0,
                    options: SearchOptions::default(),
                },
                detect: vec![DetectType::Enter],
                commands: vec![MutationCommand::Set],
            },
        );

        let order_events = registry
            .evaluate_mutation(
                &mutation("orders", "order-1", None, Some(point(52.52, 13.405)), 1),
                &|_| Vec::new(),
            )
            .unwrap();
        assert_eq!(order_events.len(), 1);
        assert_eq!(order_events[0].hook.as_deref(), Some("orders-hook"));

        let fleet_events = registry
            .evaluate_mutation(
                &mutation("fleet", "truck-1", None, Some(point(52.52, 13.405)), 2),
                &|_| Vec::new(),
            )
            .unwrap();
        assert_eq!(fleet_events.len(), 1);
        assert_eq!(fleet_events[0].hook.as_deref(), Some("fleet-hook"));
    }

    #[test]
    fn related_hooks_enqueue_once_each_without_duplicates() {
        let mut registry = GeofenceRegistry::<NativePlatform>::new();
        for name in ["fleet-hook-a", "fleet-hook-b"] {
            registry.set_hook(
                name,
                format!("https://example.invalid/{name}"),
                GeofenceDef {
                    collection: "fleet".to_owned(),
                    query: GeofenceQuery::Nearby {
                        lat: 52.52,
                        lon: 13.405,
                        meters: 100.0,
                        options: SearchOptions::default(),
                    },
                    detect: vec![DetectType::Enter],
                    commands: vec![MutationCommand::Set],
                },
            );
        }

        let first = registry
            .evaluate_mutation(
                &mutation("fleet", "truck-1", None, Some(point(52.52, 13.405)), 1),
                &|_| Vec::new(),
            )
            .unwrap();
        assert_eq!(first.len(), 2);
        let mut hook_names = first
            .iter()
            .filter_map(|event| event.hook.clone())
            .collect::<Vec<_>>();
        hook_names.sort();
        assert_eq!(
            hook_names,
            vec!["fleet-hook-a".to_owned(), "fleet-hook-b".to_owned()]
        );

        let second = registry
            .evaluate_mutation(
                &mutation("fleet", "truck-1", None, Some(point(52.5201, 13.4051)), 2),
                &|_| Vec::new(),
            )
            .unwrap();
        assert!(second.is_empty());
    }

    fn point(lat: f64, lon: f64) -> Object {
        object("", lat, lon)
    }

    fn object(id: &str, lat: f64, lon: f64) -> Object {
        Object {
            id: id.to_owned(),
            geo: GeoType::point(lat, lon),
            fields: FieldMap::new(),
            expires_at: None,
        }
    }

    fn mutation(
        collection: &str,
        id: &str,
        before: Option<Object>,
        after: Option<Object>,
        timestamp_ns: u64,
    ) -> MutationEvent {
        MutationEvent {
            command: MutationCommand::Set,
            collection: collection.to_owned(),
            id: id.to_owned(),
            before: before.map(|mut object| {
                object.id = id.to_owned();
                object
            }),
            after: after.map(|mut object| {
                object.id = id.to_owned();
                object
            }),
            timestamp_ns,
        }
    }
}
