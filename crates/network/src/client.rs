use std::collections::VecDeque;

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use glam::Vec3;
use bevy_replicon_renet2::renet2::RenetClientPlugin;
use crate::events::PlayerInputEvent;

pub struct ClientPlugin {
    pub _config: crate::config::NetworkConfig,
}

impl Plugin for ClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(RenetClientPlugin)
           .add_systems(Update, (
               client_input_system,
               client_prediction_system,
               server_reconciliation_system,
               interpolation_system,
               handle_server_events,
           ));
    }
}

#[derive(Component, Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct PredictedPosition(pub Vec3);

#[derive(Component, Debug)]
#[allow(dead_code)]
pub struct InputHistory {
    pub inputs: VecDeque<(u16, PlayerInputEvent)>,
    pub max_size: usize,
}

#[allow(dead_code)]
impl InputHistory {
    pub fn new() -> Self {
        Self {
            inputs: VecDeque::with_capacity(64),
            max_size: 64,
        }
    }

    pub fn push(&mut self, tick: u16, input: PlayerInputEvent) {
        self.inputs.push_back((tick, input));
        while self.inputs.len() > self.max_size {
            self.inputs.pop_front();
        }
    }

    pub fn get_up_to(&self, tick: u16) -> Vec<PlayerInputEvent> {
        self.inputs
            .iter()
            .filter(|(t, _)| *t <= tick)
            .map(|(_, input)| input.clone())
            .collect()
    }
}

impl Default for InputHistory {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Component, Debug)]
#[allow(dead_code)]
pub struct InterpolatedPosition {
    pub previous: Vec3,
    pub current: Vec3,
    pub alpha: f32,
}

#[allow(dead_code)]
impl InterpolatedPosition {
    pub fn new(pos: Vec3) -> Self {
        Self {
            previous: pos,
            current: pos,
            alpha: 1.0,
        }
    }
}

fn client_input_system(
    mut _commands: Commands,
) {
}

fn client_prediction_system(
    mut query: Query<(&mut PredictedPosition, &mut InputHistory)>,
) {
    for (pos, _history) in query.iter_mut() {
        let _ = pos;
    }
}

fn server_reconciliation_system(
    mut query: Query<(&mut PredictedPosition, &mut InputHistory)>,
) {
    for (pos, history) in query.iter_mut() {
        let _ = (pos, history);
    }
}

fn interpolation_system(
    mut query: Query<&mut InterpolatedPosition>,
    time: Res<bevy_time::Time>,
) {
    let delta = time.delta_secs();
    let interpolation_delay = 0.05;

    for mut interp in query.iter_mut() {
        interp.alpha += delta / interpolation_delay;
        interp.alpha = interp.alpha.min(1.0);
    }
}

fn handle_server_events() {
}
