use std::{
    any::{Any, TypeId},
    sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use bevy_app::prelude::AppTypeRegistry;
use bevy_asset::{HandleId, HandleUntyped, ReflectAsset};
use bevy_ecs::{component::ComponentId, prelude::*, world::EntityRef};
use bevy_hierarchy::{Children, Parent};
use bevy_reflect::{Reflect, ReflectFromPtr, TypeRegistry};
use egui::FontId;

use crate::{
    driver_egui::{split_world_permission, Context, InspectorEguiOverrides, InspectorUi},
    egui_utils::layout_job,
};

#[derive(Resource, Default, Clone)]
pub struct AppInspectorEguiOverrides(Arc<RwLock<InspectorEguiOverrides>>);
impl AppInspectorEguiOverrides {
    pub fn read(&self) -> RwLockReadGuard<InspectorEguiOverrides> {
        self.0.read().unwrap()
    }
    pub fn write(&self) -> RwLockWriteGuard<InspectorEguiOverrides> {
        self.0.write().unwrap()
    }
}

pub fn ui_for_world(world: &mut World, ui: &mut egui::Ui) {
    crate::setup_default_inspector_options(world);

    let type_registry = world.resource::<AppTypeRegistry>().0.clone();
    let type_registry = type_registry.read();
    let egui_overrides = world
        .get_resource_or_insert_with(AppInspectorEguiOverrides::default)
        .clone();
    let egui_overrides = egui_overrides.read();

    egui::CollapsingHeader::new("Entities").show(ui, |ui| {
        ui_for_world_entities_with(world, ui, &type_registry, &egui_overrides);
    });
    egui::CollapsingHeader::new("Resources").show(ui, |ui| {
        let mut resources: Vec<_> = type_registry
            .iter()
            .filter(|registration| registration.data::<ReflectResource>().is_some())
            .map(|registration| (registration.short_name().to_owned(), registration.type_id()))
            .collect();
        resources.sort_by(|(name_a, ..), (name_b, ..)| name_a.cmp(name_b));
        for (name, type_id) in resources {
            ui.collapsing(&name, |ui| {
                ui_for_resource_with(world, type_id, ui, &type_registry, &egui_overrides);
            });
        }
    });
    egui::CollapsingHeader::new("Assets").show(ui, |ui| {
        let mut assets: Vec<_> = type_registry
            .iter()
            .filter(|registration| registration.data::<ReflectAsset>().is_some())
            .map(|registration| (registration.short_name().to_owned(), registration.type_id()))
            .collect();
        assets.sort_by(|(name_a, ..), (name_b, ..)| name_a.cmp(name_b));
        for (name, type_id) in assets {
            ui.collapsing(&name, |ui| {
                ui_for_asset_with(world, type_id, ui, &type_registry, &egui_overrides);
            });
        }
    });
}

pub fn ui_for_resource(world: &mut World, resource_type_id: TypeId, ui: &mut egui::Ui) {
    let type_registry = world.resource::<AppTypeRegistry>().0.clone();
    let type_registry = type_registry.read();
    let egui_overrides = world
        .get_resource_or_insert_with(AppInspectorEguiOverrides::default)
        .clone();
    let egui_overrides = egui_overrides.read();

    ui_for_resource_with(world, resource_type_id, ui, &type_registry, &egui_overrides);
}

pub fn ui_for_resource_with(
    world: &mut World,
    resource_type_id: TypeId,
    ui: &mut egui::Ui,
    type_registry: &TypeRegistry,
    egui_overrides: &InspectorEguiOverrides,
) {
    crate::setup_default_inspector_options(world);

    let (no_resource_refs_world, only_resource_access_world) =
        split_world_permission(world, Some(resource_type_id));

    let mut cx = Context {
        world: Some(only_resource_access_world),
    };
    let mut env = InspectorUi::new(type_registry, egui_overrides, &mut cx, Some(short_circuit));

    // SAFETY: in the code below, the only reference to a resource is the one specified as `except` in `split_world_permission`;
    debug_assert!(no_resource_refs_world.allows_access_to(resource_type_id));
    let nrr_world = unsafe { no_resource_refs_world.get() };
    let component_id = nrr_world
        .components()
        .get_resource_id(resource_type_id)
        .unwrap();
    // SAFETY: component_id refers to the component use as the exception in `split_world_permission`,
    // `NoResourceRefsWorld` allows mutable access.
    let mut mut_untyped = unsafe {
        nrr_world
            .get_resource_unchecked_mut_by_id(component_id)
            .unwrap()
    };
    // TODO: only do this if changed
    mut_untyped.set_changed();

    let reflect_from_ptr = type_registry
        .get_type_data::<ReflectFromPtr>(resource_type_id)
        .unwrap();
    assert_eq!(reflect_from_ptr.type_id(), resource_type_id);
    // SAFETY: value type is the type of the `ReflectFromPtr`
    let value = unsafe { reflect_from_ptr.as_reflect_ptr_mut(mut_untyped.into_inner()) };
    let _changed = env.ui_for_reflect(value, ui, egui::Id::new(resource_type_id));
}

pub fn ui_for_asset_with(
    world: &mut World,
    asset_type_id: TypeId,
    ui: &mut egui::Ui,
    type_registry: &TypeRegistry,
    egui_overrides: &InspectorEguiOverrides,
) {
    crate::setup_default_inspector_options(world);

    let registration = type_registry.get(asset_type_id).unwrap();
    let reflect_asset = registration.data::<ReflectAsset>().unwrap();

    let mut ids: Vec<_> = reflect_asset.ids(world).collect();
    ids.sort();

    let (no_resource_refs_world, only_resource_access_world) =
        split_world_permission(world, Some(reflect_asset.assets_resource_type_id()));
    let mut cx = Context {
        world: Some(only_resource_access_world),
    };

    // SAFETY: in the code below, the only reference to a resource is the one specified as `except` in `split_world_permission`
    let nrr_world = unsafe { no_resource_refs_world.get() };

    for handle_id in ids {
        let id = egui::Id::new(handle_id);
        egui::CollapsingHeader::new(format!("Handle({id:?})"))
            .id_source(id)
            .show(ui, |ui| {
                // SAFETY: the `NoResourceRefs` allows mutable access, and in particular to the resource assets resource of the asset
                // since we specified it as the exception
                let value = unsafe {
                    reflect_asset
                        .get_unchecked_mut(nrr_world, HandleUntyped::weak(handle_id))
                        .unwrap()
                };

                let mut env =
                    InspectorUi::new(type_registry, egui_overrides, &mut cx, Some(short_circuit));
                env.ui_for_reflect(value, ui, id);
            });
    }
}

pub fn ui_for_world_entities(world: &mut World, ui: &mut egui::Ui) {
    let type_registry = world.resource::<AppTypeRegistry>().0.clone();
    let type_registry = type_registry.read();
    let egui_overrides = world
        .get_resource_or_insert_with(AppInspectorEguiOverrides::default)
        .clone();
    let egui_overrides = egui_overrides.read();

    ui_for_world_entities_with(world, ui, &type_registry, &egui_overrides);
}

pub fn ui_for_world_entities_with(
    world: &mut World,
    ui: &mut egui::Ui,
    type_registry: &TypeRegistry,
    egui_overrides: &InspectorEguiOverrides,
) {
    crate::setup_default_inspector_options(world);

    let mut root_entities = world.query_filtered::<Entity, Without<Parent>>();
    let mut entities = root_entities.iter(world).collect::<Vec<_>>();
    entities.sort();

    let id = egui::Id::new("world ui");
    for entity in entities {
        ui_for_entity_with(
            world,
            entity,
            ui,
            id.with(entity),
            type_registry,
            egui_overrides,
        );
    }
}

pub fn ui_for_entity(world: &mut World, entity: Entity, ui: &mut egui::Ui) {
    let type_registry = world.resource::<AppTypeRegistry>().0.clone();
    let type_registry = type_registry.read();
    let egui_overrides = world
        .get_resource_or_insert_with(AppInspectorEguiOverrides::default)
        .clone();
    let egui_overrides = egui_overrides.read();

    ui_for_entity_with(
        world,
        entity,
        ui,
        egui::Id::new(entity),
        &type_registry,
        &egui_overrides,
    );
}

pub fn ui_for_entity_with(
    world: &mut World,
    entity: Entity,
    ui: &mut egui::Ui,
    id: egui::Id,
    type_registry: &TypeRegistry,
    egui_overrides: &InspectorEguiOverrides,
) {
    let entity_name = guess_entity_name::entity_name(world, entity);

    egui::CollapsingHeader::new(entity_name)
        .id_source(id)
        .show(ui, |ui| {
            ui_for_entity_components(world, entity, ui, id, type_registry, egui_overrides);

            let children = world
                .get::<Children>(entity)
                .map(|children| children.iter().copied().collect::<Vec<_>>());
            if let Some(children) = children {
                if !children.is_empty() {
                    ui.label("Children");
                    for &child in children.iter() {
                        let id = id.with(child);
                        ui_for_entity_with(world, child, ui, id, type_registry, egui_overrides);
                    }
                }
            }
        });
}

fn ui_for_entity_components(
    world: &mut World,
    entity: Entity,
    ui: &mut egui::Ui,
    id: egui::Id,
    type_registry: &TypeRegistry,
    egui_overrides: &InspectorEguiOverrides,
) {
    let entity_ref = match world.get_entity(entity) {
        Some(entity) => entity,
        None => {
            error_message_entity_does_not_exist(ui, entity);
            return;
        }
    };
    let components = components_of_entity(entity_ref, world);

    let (no_resource_refs_world, only_resource_access_world) = split_world_permission(world, None);
    let mut cx = Context {
        world: Some(only_resource_access_world),
    };
    // SAFETY: in the code below, no references to resources are held
    let nrr_world = unsafe { no_resource_refs_world.get() };

    for (name, component_id, type_id, size) in components {
        let id = id.with(component_id);
        egui::CollapsingHeader::new(&name)
            .id_source(id)
            .show(ui, |ui| {
                // SAFETY: mutable access is allowed through `NoResourceRefsWorld`, just not to resources
                let value = unsafe {
                    nrr_world
                        .entity(entity)
                        .get_unchecked_mut_by_id(component_id)
                        .unwrap()
                };

                if size == 0 {
                    return;
                }

                let type_id = match type_id {
                    Some(type_id) => type_id,
                    None => return error_message_no_type_id(ui, &name),
                };
                let reflect_from_ptr = match type_registry.get_type_data::<ReflectFromPtr>(type_id)
                {
                    Some(type_id) => type_id,
                    None => return error_message_no_reflect_from_ptr(ui, &name),
                };
                assert_eq!(reflect_from_ptr.type_id(), type_id);
                // SAFETY: value is of correct type, as checked above
                let value = unsafe { reflect_from_ptr.as_reflect_ptr_mut(value.into_inner()) };

                InspectorUi::new(type_registry, egui_overrides, &mut cx, Some(short_circuit))
                    .ui_for_reflect(value, ui, id.with(component_id));
            });
    }
}

fn components_of_entity(
    entity_ref: EntityRef,
    world: &World,
) -> Vec<(String, ComponentId, Option<TypeId>, usize)> {
    let archetype = entity_ref.archetype();
    let mut components: Vec<_> = archetype
        .components()
        .map(|component_id| {
            let info = world.components().get_info(component_id).unwrap();
            let name = pretty_type_name::pretty_type_name_str(info.name());

            (name, component_id, info.type_id(), info.layout().size())
        })
        .collect();
    components.sort_by(|(name_a, ..), (name_b, ..)| name_a.cmp(name_b));
    components
}

fn error_message_no_type_id(ui: &mut egui::Ui, component_name: &str) {
    let job = layout_job(&[
        (FontId::monospace(14.0), component_name),
        (
            FontId::default(),
            " is not backed by a rust type, so it cannot be displayed.",
        ),
    ]);

    ui.label(job);
}

fn error_message_no_reflect_from_ptr(ui: &mut egui::Ui, type_name: &str) {
    let job = layout_job(&[
        (FontId::monospace(14.0), type_name),
        (FontId::default(), " has no "),
        (FontId::monospace(14.0), "ReflectFromPtr"),
        (FontId::default(), " type data, so it cannot be displayed"),
    ]);

    ui.label(job);
}

fn error_message_entity_does_not_exist(ui: &mut egui::Ui, entity: Entity) {
    let job = layout_job(&[
        (FontId::default(), "Entity "),
        (FontId::monospace(14.0), &format!("{entity:?}")),
        (FontId::default(), " does not exist."),
    ]);

    ui.label(job);
}
fn error_message_no_world_in_context(ui: &mut egui::Ui, type_name: &str) {
    let job = layout_job(&[
        (FontId::monospace(14.0), type_name),
        (FontId::default(), " needs the bevy world in the "),
        (FontId::monospace(14.0), "InspectorUi"),
        (
            FontId::default(),
            " context to provide meaningful information.",
        ),
    ]);

    ui.label(job);
}
fn error_message_dead_asset_handle(ui: &mut egui::Ui, handle: HandleId) {
    let job = layout_job(&[
        (FontId::default(), "Handle "),
        (FontId::monospace(14.0), &format!("{:?}", handle)),
        (FontId::default(), " points to no asset."),
    ]);

    ui.label(job);
}

mod guess_entity_name {
    use bevy_core::Name;
    use bevy_ecs::{prelude::*, world::EntityRef};

    /// Guesses an appropriate entity name like `Light (6)` or falls back to `Entity (8)`
    pub fn entity_name(world: &World, entity: Entity) -> String {
        match world.get_entity(entity) {
            Some(entity) => guess_entity_name_inner(entity),
            None => format!("Entity {} (inexistent)", entity.id()),
        }
    }

    fn guess_entity_name_inner(entity: EntityRef) -> String {
        if let Some(name) = entity.get::<Name>() {
            return name.as_str().to_string();
        }

        let id = entity.id().id();

        format!("Entity ({:?})", id)
    }
}

// Short circuit reflect UI in cases where we have better information available through the world (e.g. handles to assets)
fn short_circuit(
    env: &mut InspectorUi,
    value: &mut dyn Reflect,
    ui: &mut egui::Ui,
    id: egui::Id,
    options: &dyn Any,
) -> Option<bool> {
    if let Some(reflect_handle) = env
        .type_registry
        .get_type_data::<bevy_asset::ReflectHandle>(Any::type_id(value))
    {
        let handle = reflect_handle
            .downcast_handle_untyped(value.as_any())
            .unwrap();
        let handle_id = handle.id;
        let reflect_asset = env
            .type_registry
            .get_type_data::<bevy_asset::ReflectAsset>(reflect_handle.asset_type_id())
            .unwrap();

        let world = match &env.context.world {
            Some(world) => world,
            None => {
                error_message_no_world_in_context(ui, value.type_name());
                return Some(false);
            }
        };
        assert!(!world.forbids_access_to(reflect_asset.assets_resource_type_id()));
        // SAFETY: the following code only accesses resources through the world (namely `Assets<T>`)
        let ora_world = unsafe { world.get() };
        // SAFETY: the `OnlyResourceAccessWorld` allows mutable access (except for the `except_resource`),
        // and we create only one reference to an asset at the same time.
        let asset_value = unsafe { reflect_asset.get_unchecked_mut(ora_world, handle) };
        let asset_value = match asset_value {
            Some(value) => value,
            None => {
                error_message_dead_asset_handle(ui, handle_id);
                return Some(false);
            }
        };

        let more_restricted_world = env.context.world.as_ref().map(|world| {
            // SAFETY: while this world is active, the only live reference to a resource through the `world` is
            // through the `assets_resource_type_id`.
            unsafe { world.with_more_restriction(reflect_asset.assets_resource_type_id()) }
        });

        let mut restricted_env = InspectorUi {
            type_registry: env.type_registry,
            egui_overrides: env.egui_overrides,
            context: &mut Context {
                world: more_restricted_world,
            },
            short_circuit: env.short_circuit,
        };
        return Some(restricted_env.ui_for_reflect_with_options(
            asset_value,
            ui,
            id.with("asset"),
            options,
        ));
    }

    None
}
