use super::super::{graph::storage::GraphStorage, RenderGraphError, RenderNodeId};

#[test]
fn allocation_returns_valid_typed_ids() {
    let mut storage = GraphStorage::<&'static str, RenderNodeId>::new();

    let a = storage.allocate("a").unwrap();
    let b = storage.allocate("b").unwrap();

    assert!(a.is_valid());
    assert!(b.is_valid());
    assert_ne!(a, b);
    assert_eq!(storage.len(), 2);
    assert_eq!(storage.get(a).unwrap(), &"a");
    assert_eq!(storage.get(b).unwrap(), &"b");
}

#[test]
fn freeing_invalidates_the_id() {
    let mut storage = GraphStorage::<&'static str, RenderNodeId>::new();
    let id = storage.allocate("dead").unwrap();

    assert_eq!(storage.free(id).unwrap(), "dead");
    assert!(!storage.is_allocated(id));
    assert_eq!(
        storage.get(id).unwrap_err(),
        RenderGraphError::InvalidId {
            kind: "graph storage item",
            raw: id.raw()
        }
    );
}

#[test]
fn usage_order_is_dense_over_live_entries() {
    let mut storage = GraphStorage::<&'static str, RenderNodeId>::new();
    let a = storage.allocate("a").unwrap();
    let b = storage.allocate("b").unwrap();
    let c = storage.allocate("c").unwrap();

    assert_eq!(
        storage.ids_in_usage_order().collect::<Vec<_>>(),
        vec![a, b, c]
    );
    assert_eq!(storage.usage_index(a), Some(0));
    assert_eq!(storage.usage_index(b), Some(1));
    assert_eq!(storage.usage_index(c), Some(2));
}

#[test]
fn freeing_swap_removes_without_stale_live_entries() {
    let mut storage = GraphStorage::<&'static str, RenderNodeId>::new();
    let a = storage.allocate("a").unwrap();
    let b = storage.allocate("b").unwrap();
    let c = storage.allocate("c").unwrap();

    assert_eq!(storage.free(b).unwrap(), "b");

    assert_eq!(storage.len(), 2);
    assert!(!storage.is_allocated(b));
    assert_eq!(storage.ids_in_usage_order().collect::<Vec<_>>(), vec![a, c]);
    assert_eq!(storage.usage_index(a), Some(0));
    assert_eq!(storage.usage_index(c), Some(1));
    assert_eq!(storage.usage_index(b), None);
}

#[test]
fn iteration_uses_usage_order_not_raw_slot_order() {
    let mut storage = GraphStorage::<&'static str, RenderNodeId>::new();
    let a = storage.allocate("a").unwrap();
    let b = storage.allocate("b").unwrap();
    let c = storage.allocate("c").unwrap();
    let d = storage.allocate("d").unwrap();

    storage.free(b).unwrap();
    let e = storage.allocate("e").unwrap();

    assert_eq!(
        storage.ids_in_usage_order().collect::<Vec<_>>(),
        vec![a, d, c, e]
    );
    assert_eq!(
        storage.iter().map(|(_, value)| *value).collect::<Vec<_>>(),
        vec!["a", "d", "c", "e"]
    );
    assert_eq!(storage.id_by_usage_index(1).unwrap(), d);
}

#[test]
fn mutable_access_by_id_updates_the_record() {
    let mut storage = GraphStorage::<String, RenderNodeId>::new();
    let id = storage.allocate(String::from("old")).unwrap();

    storage.get_mut(id).unwrap().push_str("-new");

    assert_eq!(storage.get(id).unwrap(), "old-new");
}

#[test]
fn reset_clears_all_live_ids_and_usage_order() {
    let mut storage = GraphStorage::<&'static str, RenderNodeId>::new();
    let a = storage.allocate("a").unwrap();
    let b = storage.allocate("b").unwrap();

    storage.clear();

    assert!(storage.is_empty());
    assert!(!storage.is_allocated(a));
    assert!(!storage.is_allocated(b));
    assert_eq!(storage.ids_in_usage_order().count(), 0);
}
