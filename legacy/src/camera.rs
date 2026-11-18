//! Стратегическая камера: перемещение (клавиатура / мышь) и зум.

use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::prelude::*;

#[derive(Component)]
pub struct MainCamera;

/// Системы камеры. Другие модули могут упорядочиваться `.after(CameraSet)`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct CameraSet;

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_camera).add_systems(
            Update,
            (camera_pan_keyboard, camera_pan_mouse, camera_zoom).in_set(CameraSet),
        );
    }
}

fn spawn_camera(mut commands: Commands) {
    let mut camera = Camera2dBundle::default();
    camera.projection.scale = 2.5;
    commands.spawn((camera, MainCamera));
}

fn camera_pan_keyboard(
    keyboard_input: Res<ButtonInput<KeyCode>>,
    mut query: Query<(&mut Transform, &OrthographicProjection), With<MainCamera>>,
    time: Res<Time>,
) {
    let Ok((mut transform, projection)) = query.get_single_mut() else { return };

    let mut direction = Vec3::ZERO;
    if keyboard_input.pressed(KeyCode::KeyW) || keyboard_input.pressed(KeyCode::ArrowUp) {
        direction.y += 1.0;
    }
    if keyboard_input.pressed(KeyCode::KeyS) || keyboard_input.pressed(KeyCode::ArrowDown) {
        direction.y -= 1.0;
    }
    if keyboard_input.pressed(KeyCode::KeyA) || keyboard_input.pressed(KeyCode::ArrowLeft) {
        direction.x -= 1.0;
    }
    if keyboard_input.pressed(KeyCode::KeyD) || keyboard_input.pressed(KeyCode::ArrowRight) {
        direction.x += 1.0;
    }

    if direction != Vec3::ZERO {
        let speed = 900.0_f32 * projection.scale;
        transform.translation += direction.normalize() * speed * time.delta_seconds();
    }
}

fn camera_pan_mouse(
    mouse_button_input: Res<ButtonInput<MouseButton>>,
    mut mouse_motion_events: EventReader<MouseMotion>,
    mut query: Query<(&mut Transform, &OrthographicProjection), With<MainCamera>>,
) {
    if mouse_button_input.pressed(MouseButton::Middle) || mouse_button_input.pressed(MouseButton::Right) {
        let Ok((mut transform, projection)) = query.get_single_mut() else { return };
        for event in mouse_motion_events.read() {
            transform.translation.x -= event.delta.x * projection.scale;
            transform.translation.y += event.delta.y * projection.scale;
        }
    } else {
        mouse_motion_events.clear();
    }
}

fn camera_zoom(
    mut mouse_wheel_events: EventReader<MouseWheel>,
    mut query: Query<&mut OrthographicProjection, With<MainCamera>>,
) {
    let Ok(mut projection) = query.get_single_mut() else { return };

    for event in mouse_wheel_events.read() {
        let zoom_factor = if event.y > 0.0 { 0.85_f32 } else { 1.15_f32 };
        projection.scale = (projection.scale * zoom_factor).clamp(0.2, 6.0);
    }
}
