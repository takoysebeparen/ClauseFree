mod camera;
mod clock;
mod division;
mod economy;
mod map;

use std::path::PathBuf;

use bevy::prelude::*;
use bevy::render::settings::{Backends, RenderCreation, WgpuSettings};
use bevy::render::RenderPlugin;
use bevy_egui::EguiPlugin;

use camera::CameraPlugin;
use clock::ClockPlugin;
use division::DivisionPlugin;
use economy::EconomyPlugin;
use map::MapPlugin;

const HOI4_PATH: &str = "D:\\SteamLibrary\\steamapps\\common\\Hearts of Iron IV";
pub const WINDOW_TITLE: &str = "ClauseFree v0.1";

fn main() {
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: WINDOW_TITLE.into(),
                        resolution: (1280.0_f32, 720.0_f32).into(),
                        ..default()
                    }),
                    ..default()
                })
                .set(RenderPlugin {
                    render_creation: RenderCreation::Automatic(WgpuSettings {
                        // Строго Vulkan, без DX12
                        backends: Some(Backends::VULKAN),
                        ..default()
                    }),
                    ..default()
                }),
        )
        .add_plugins(EguiPlugin)
        .add_plugins((CameraPlugin, MapPlugin { root: PathBuf::from(HOI4_PATH) }, ClockPlugin, DivisionPlugin, EconomyPlugin))
        .run();
}
