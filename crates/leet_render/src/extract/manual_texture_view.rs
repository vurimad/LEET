use crate::{Extract, ManualTextureViews};
use bevy_ecs::prelude::{Res, ResMut};
use std::ops::Deref;

pub(crate) fn extract_manual_texture_views(
    mut extracted_manual_texture_views: ResMut<ManualTextureViews>,
    manual_texture_views: Extract<Option<Res<ManualTextureViews>>>,
) {
    if let Some(manual_texture_views) = manual_texture_views.as_ref() {
        *extracted_manual_texture_views = manual_texture_views.deref().clone();
    } else {
        extracted_manual_texture_views.clear();
    }
}
