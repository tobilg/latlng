use super::*;

pub(crate) fn collection_handle_from_catalog<P: Platform>(
    collections: &P::RwLock<HashMap<String, CollectionHandle<P>>>,
    collection: &str,
) -> Option<CollectionHandle<P>> {
    P::read(collections).get(collection).cloned()
}

pub(crate) fn ensure_collection_cell_in_catalog<P: Platform>(
    collections: &P::RwLock<HashMap<String, CollectionHandle<P>>>,
    collection: &str,
) -> CollectionHandle<P> {
    if let Some(handle) = collection_handle_from_catalog::<P>(collections, collection) {
        return handle;
    }

    let mut all = P::write(collections);
    all.entry(collection.to_owned())
        .or_insert_with(|| P::shared(P::new_rwlock(VersionedCollection::new(collection))))
        .clone()
}
