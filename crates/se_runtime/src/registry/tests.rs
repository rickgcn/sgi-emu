use std::cell::RefCell;
use std::rc::Rc;

use super::*;

struct TestComponent {
    id: ComponentId,
    name: &'static str,
    resets: Rc<RefCell<Vec<u64>>>,
}

impl TestComponent {
    fn new(id: u64, name: &'static str, resets: Rc<RefCell<Vec<u64>>>) -> Self {
        Self {
            id: ComponentId::new(id),
            name,
            resets,
        }
    }
}

impl Component for TestComponent {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn name(&self) -> &str {
        self.name
    }

    fn reset(&mut self) {
        self.resets.borrow_mut().push(self.id.get());
    }
}

#[test]
fn inserts_queries_and_removes_components() {
    let resets = Rc::new(RefCell::new(Vec::new()));
    let mut registry = ComponentRegistry::new();

    registry
        .insert(Box::new(TestComponent::new(2, "device", resets)))
        .unwrap();

    assert_eq!(registry.len(), 1);
    assert!(registry.contains(ComponentId::new(2)));
    assert_eq!(registry.get(ComponentId::new(2)).unwrap().name(), "device");
    assert!(registry.get_mut(ComponentId::new(2)).is_some());

    let removed = registry.remove(ComponentId::new(2)).unwrap();
    assert_eq!(removed.id(), ComponentId::new(2));
    assert!(registry.is_empty());
}

#[test]
fn rejects_duplicate_component_ids() {
    let resets = Rc::new(RefCell::new(Vec::new()));
    let mut registry = ComponentRegistry::new();

    registry
        .insert(Box::new(TestComponent::new(4, "first", resets.clone())))
        .unwrap();

    assert_eq!(
        registry.insert(Box::new(TestComponent::new(4, "second", resets))),
        Err(RegistryError::DuplicateComponent {
            id: ComponentId::new(4),
        })
    );
}

#[test]
fn reset_all_uses_stable_component_id_order() {
    let resets = Rc::new(RefCell::new(Vec::new()));
    let mut registry = ComponentRegistry::new();

    registry
        .insert(Box::new(TestComponent::new(30, "third", resets.clone())))
        .unwrap();
    registry
        .insert(Box::new(TestComponent::new(10, "first", resets.clone())))
        .unwrap();
    registry
        .insert(Box::new(TestComponent::new(20, "second", resets.clone())))
        .unwrap();

    registry.reset_all();

    assert_eq!(*resets.borrow(), vec![10, 20, 30]);
}

#[test]
fn typed_lookups_return_the_requested_component() {
    let resets = Rc::new(RefCell::new(Vec::new()));
    let mut registry = ComponentRegistry::new();
    let id = ComponentId::new(40);
    registry
        .insert(Box::new(TestComponent::new(40, "typed", resets)))
        .unwrap();

    assert_eq!(
        registry.get_typed::<TestComponent>(id).unwrap().name(),
        "typed"
    );
    registry.get_typed_mut::<TestComponent>(id).unwrap().name = "changed";
    assert_eq!(
        registry.get_typed::<TestComponent>(id).unwrap().name(),
        "changed"
    );
}

#[test]
fn typed_lookups_distinguish_missing_components_and_type_mismatches() {
    #[derive(Default)]
    struct OtherComponent;

    impl Component for OtherComponent {
        fn id(&self) -> ComponentId {
            ComponentId::new(50)
        }

        fn name(&self) -> &str {
            "other"
        }

        fn reset(&mut self) {}
    }

    let resets = Rc::new(RefCell::new(Vec::new()));
    let mut registry = ComponentRegistry::new();
    registry
        .insert(Box::new(TestComponent::new(50, "typed", resets)))
        .unwrap();

    assert_eq!(
        registry
            .get_typed::<TestComponent>(ComponentId::new(51))
            .err()
            .unwrap(),
        RegistryLookupError::MissingComponent {
            id: ComponentId::new(51),
        }
    );
    assert_eq!(
        registry
            .get_typed::<OtherComponent>(ComponentId::new(50))
            .err()
            .unwrap(),
        RegistryLookupError::TypeMismatch {
            id: ComponentId::new(50),
            expected: core::any::type_name::<OtherComponent>(),
        }
    );
}

#[test]
fn resolved_slots_are_typed_and_invalidated_by_topology_changes() {
    let resets = Rc::new(RefCell::new(Vec::new()));
    let mut registry = ComponentRegistry::new();
    let id = ComponentId::new(20);
    registry
        .insert(Box::new(TestComponent::new(20, "slotted", resets.clone())))
        .unwrap();

    let slot = registry.resolve::<TestComponent>(id).unwrap();
    assert_eq!(slot.id(), id);
    assert_eq!(registry.get_resolved(slot).unwrap().name(), "slotted");
    registry.get_resolved_mut(slot).unwrap().name = "changed";
    assert_eq!(registry.get_resolved(slot).unwrap().name(), "changed");

    registry
        .insert(Box::new(TestComponent::new(10, "inserted", resets)))
        .unwrap();
    assert_eq!(
        registry.get_resolved(slot).err().unwrap(),
        RegistryLookupError::StaleSlot { id }
    );
}
