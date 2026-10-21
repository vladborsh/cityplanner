//! Sound effects (T-189). The client listens to what the player did and what the sim said about
//! it — the commands it submitted and whether each was accepted — and queues a sound for it; it
//! never changes the sim. The mapping, the volume, the mute switch and the rate limit are plain
//! functions of values (testable without a device); [`SoundPlugin`] plays the queue through
//! Bevy's audio. `--hidden` runs and tests never open an audio device: the desktop client turns
//! Bevy's `AudioPlugin` off then, and this plugin is only added with the UI.
//!
//! The sounds are self-made (`tools/assets/gen_sounds.py`, `assets/sounds/*.wav`).

use std::collections::BTreeMap;

use bevy::audio::{AudioPlayer, PlaybackSettings, Volume};
use bevy::prelude::*;
use cp_building::BuildingCommand;
use cp_net::NetCommand;
use cp_sim::SimCommand;
use cp_utility::UtilityCommand;
use cp_zone::ZoneCommand;
use serde::{Deserialize, Serialize};

/// Most times one sound plays a second.
pub const MAX_PER_SECOND: f64 = 10.0;
/// The pitch of a played sound varies by this share either way.
pub const PITCH_SPREAD: f32 = 0.06;

/// A sound the client can play.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Sound {
    RoadBuilt,
    ZonePainted,
    BuildingPlaced,
    UtilityBuilt,
    Demolish,
    Invalid,
    UiClick,
    /// The advisor's snackbar (T-188), by severity.
    AdvisorInfo,
    AdvisorWarn,
    AdvisorAlert,
}

impl Sound {
    pub const ALL: [Sound; 10] = [
        Self::RoadBuilt,
        Self::ZonePainted,
        Self::BuildingPlaced,
        Self::UtilityBuilt,
        Self::Demolish,
        Self::Invalid,
        Self::UiClick,
        Self::AdvisorInfo,
        Self::AdvisorWarn,
        Self::AdvisorAlert,
    ];

    /// The file under `assets/`.
    pub fn file(self) -> &'static str {
        match self {
            Self::RoadBuilt => "sounds/road_built.wav",
            Self::ZonePainted => "sounds/zone_painted.wav",
            Self::BuildingPlaced => "sounds/building_placed.wav",
            Self::UtilityBuilt => "sounds/utility_built.wav",
            Self::Demolish => "sounds/demolish.wav",
            Self::Invalid => "sounds/error.wav",
            Self::UiClick => "sounds/ui_click.wav",
            Self::AdvisorInfo => "sounds/advisor_info.wav",
            Self::AdvisorWarn => "sounds/advisor_warn.wav",
            Self::AdvisorAlert => "sounds/advisor_alert.wav",
        }
    }
}

/// The sound of a command the sim accepted, if it has one. One command is one click (a chain
/// of road segments is one `BuildRoad` per click, so one sound).
pub fn sound_for_command(command: &SimCommand) -> Option<Sound> {
    match command {
        SimCommand::Net(
            NetCommand::BuildRoad { .. }
            | NetCommand::BuildPlan { .. }
            | NetCommand::UpgradeSegment { .. }
            | NetCommand::UpgradePart { .. }
            | NetCommand::FlipSegment { .. },
        ) => Some(Sound::RoadBuilt),
        SimCommand::Net(NetCommand::RemoveSegment { .. } | NetCommand::RemovePart { .. }) => {
            Some(Sound::Demolish)
        }
        SimCommand::Zone(ZoneCommand::PaintZones { .. }) => Some(Sound::ZonePainted),
        SimCommand::Building(BuildingCommand::Place { .. }) => Some(Sound::BuildingPlaced),
        SimCommand::Building(BuildingCommand::Demolish { .. }) => Some(Sound::Demolish),
        SimCommand::Utility(UtilityCommand::BuildLink { .. }) => Some(Sound::UtilityBuilt),
        SimCommand::Utility(UtilityCommand::RemoveLink { .. }) => Some(Sound::Demolish),
        _ => None,
    }
}

/// Volumes and the mute switch (`settings.ron`, the Settings window).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    pub master: f32,
    pub effects: f32,
    pub mute: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            master: 1.0,
            // About 60 % for the effects (T-189).
            effects: 0.6,
            mute: false,
        }
    }
}

impl AudioSettings {
    /// The gain of an effect: nothing when muted.
    pub fn gain(&self) -> f32 {
        if self.mute {
            0.0
        } else {
            self.master.clamp(0.0, 1.0) * self.effects.clamp(0.0, 1.0)
        }
    }

    /// Keeps hand-edited values in `0..=1`.
    pub fn sanitized(mut self) -> Self {
        for v in [&mut self.master, &mut self.effects] {
            *v = if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.6
            };
        }
        self
    }
}

/// The sounds waiting to be played, with the rate limit and the pitch dice. The dice are the
/// client's own (a small xorshift), never the sim's RNG.
#[derive(Resource, Debug)]
pub struct SoundQueue {
    /// `(sound, pitch)` in the order they came.
    pending: Vec<(Sound, f32)>,
    last: BTreeMap<Sound, f64>,
    state: u32,
}

impl Default for SoundQueue {
    fn default() -> Self {
        Self {
            pending: Vec::new(),
            last: BTreeMap::new(),
            state: 0x9E37_79B9,
        }
    }
}

impl SoundQueue {
    fn next_unit(&mut self) -> f32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Queues `sound` at time `now` (seconds) unless muted or the same sound played less than
    /// `1 / MAX_PER_SECOND` ago. Returns whether it was queued.
    pub fn push(&mut self, sound: Sound, now: f64, settings: &AudioSettings) -> bool {
        if settings.gain() <= 0.0 {
            return false;
        }
        if self
            .last
            .get(&sound)
            .is_some_and(|t| now - t < 1.0 / MAX_PER_SECOND)
        {
            return false;
        }
        self.last.insert(sound, now);
        let pitch = 1.0 + (self.next_unit() - 0.5) * 2.0 * PITCH_SPREAD;
        self.pending.push((sound, pitch));
        true
    }

    pub fn take(&mut self) -> Vec<(Sound, f32)> {
        std::mem::take(&mut self.pending)
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

/// The chime of the advisor's snackbar (T-188) by its severity.
pub fn advisor_sound(severity: cp_ui::advisor::Severity) -> Sound {
    use cp_ui::advisor::Severity;
    match severity {
        Severity::Info => Sound::AdvisorInfo,
        Severity::Warn => Sound::AdvisorWarn,
        Severity::Critical => Sound::AdvisorAlert,
    }
}

/// A snackbar appeared: its chime.
fn advisor_chime(
    mut shown: MessageReader<cp_ui::advisor::AdvisorShown>,
    time: Res<Time<Real>>,
    prefs: Res<AudioPrefsRes>,
    mut queue: ResMut<SoundQueue>,
) {
    for m in shown.read() {
        queue.push(advisor_sound(m.severity), time.elapsed_secs_f64(), &prefs.0);
    }
}

/// What the frame's accepted and refused commands sound like: one sound per command, an
/// "invalid" for a refused one that is not just a repeat.
pub fn queue_outcomes(
    queue: &mut SoundQueue,
    commands: &[SimCommand],
    accepted: &[bool],
    now: f64,
    settings: &AudioSettings,
) {
    for (command, ok) in commands.iter().zip(accepted) {
        let sound = if *ok {
            sound_for_command(command)
        } else if sound_for_command(command).is_some() {
            Some(Sound::Invalid)
        } else {
            None
        };
        if let Some(sound) = sound {
            queue.push(sound, now, settings);
        }
    }
}

/// The settings the sounds play with, kept in a resource the Settings window edits.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct AudioPrefsRes(pub AudioSettings);

#[derive(Resource, Default)]
struct SoundHandles(BTreeMap<Sound, Handle<AudioSource>>);

/// Loads the files once and plays the queue.
fn play_queue(
    mut commands: Commands,
    assets: Option<Res<AssetServer>>,
    prefs: Res<AudioPrefsRes>,
    mut queue: ResMut<SoundQueue>,
    mut handles: ResMut<SoundHandles>,
) {
    let Some(assets) = assets else {
        queue.take();
        return;
    };
    if handles.0.is_empty() {
        for s in Sound::ALL {
            handles.0.insert(s, assets.load(s.file()));
        }
    }
    let gain = prefs.0.gain();
    for (sound, pitch) in queue.take() {
        if gain <= 0.0 {
            continue;
        }
        if let Some(h) = handles.0.get(&sound) {
            commands.spawn((
                AudioPlayer::new(h.clone()),
                PlaybackSettings::DESPAWN
                    .with_volume(Volume::Linear(gain))
                    .with_speed(pitch),
            ));
        }
    }
}

/// A click on the UI (a panel, a button, a slider) ticks softly.
fn ui_click_sound(
    time: Res<Time<Real>>,
    mouse: Res<ButtonInput<MouseButton>>,
    capture: Res<cp_render::UiCapture>,
    prefs: Res<AudioPrefsRes>,
    mut queue: ResMut<SoundQueue>,
) {
    if mouse.just_pressed(MouseButton::Left) && capture.pointer_over_ui {
        queue.push(Sound::UiClick, time.elapsed_secs_f64(), &prefs.0);
    }
}

pub struct SoundPlugin;

impl Plugin for SoundPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SoundQueue>()
            .init_resource::<SoundHandles>()
            .init_resource::<AudioPrefsRes>()
            .add_systems(Update, (ui_click_sound, advisor_chime, play_queue).chain());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_net::RoadPath;

    fn db() -> cp_content::ContentDb {
        cp_content::ContentDb::load_dir(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/content"
        ))
        .unwrap()
    }

    fn build_road() -> SimCommand {
        SimCommand::Net(NetCommand::BuildRoad {
            road: db().road_id("basic_2l").unwrap(),
            path: RoadPath::Straight {
                from: Vec2::ZERO,
                to: Vec2::X,
            },
            elevation: cp_net::ElevationMode::Ground,
        })
    }

    #[test]
    fn commands_map_to_their_sounds() {
        assert_eq!(sound_for_command(&build_road()), Some(Sound::RoadBuilt));
        let zone = SimCommand::Zone(ZoneCommand::PaintZones {
            cells: Vec::new(),
            zone: cp_content::ZoneType::ResLow,
        });
        assert_eq!(sound_for_command(&zone), Some(Sound::ZonePainted));
        let demolish = SimCommand::Building(BuildingCommand::Demolish {
            building: cp_core::Id::new(0, 0),
        });
        assert_eq!(sound_for_command(&demolish), Some(Sound::Demolish));
        let place = SimCommand::Building(BuildingCommand::Place {
            def: db().building_id("bus_depot").unwrap(),
            pos: Vec2::ZERO,
            rot: 0.0,
        });
        assert_eq!(sound_for_command(&place), Some(Sound::BuildingPlaced));
        let pipe = SimCommand::Utility(UtilityCommand::BuildLink {
            kind: cp_content::UtilityKind::Water,
            path: Vec::new(),
        });
        assert_eq!(sound_for_command(&pipe), Some(Sound::UtilityBuilt));
        // Changing the speed is not a build.
        let speed = SimCommand::SetSpeed {
            speed: cp_core::SimSpeed::X2,
        };
        assert_eq!(sound_for_command(&speed), None);
        // Every sound has its own file, and a file the assets hold.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let mut files: Vec<_> = Sound::ALL.iter().map(|s| s.file()).collect();
        for f in &files {
            assert!(root.join(f).exists(), "{f}");
        }
        files.sort();
        files.dedup();
        assert_eq!(files.len(), Sound::ALL.len());
        use cp_ui::advisor::Severity;
        assert_eq!(advisor_sound(Severity::Info), Sound::AdvisorInfo);
        assert_eq!(advisor_sound(Severity::Warn), Sound::AdvisorWarn);
        assert_eq!(advisor_sound(Severity::Critical), Sound::AdvisorAlert);
    }

    #[test]
    fn refused_builds_say_invalid_and_other_refusals_stay_quiet() {
        let s = AudioSettings::default();
        let mut q = SoundQueue::default();
        queue_outcomes(
            &mut q,
            &[
                build_road(),
                build_road(),
                SimCommand::SetSpeed {
                    speed: cp_core::SimSpeed::X2,
                },
            ],
            &[true, false, false],
            0.0,
            &s,
        );
        let got: Vec<Sound> = q.take().into_iter().map(|(s, _)| s).collect();
        assert_eq!(got, vec![Sound::RoadBuilt, Sound::Invalid]);
    }

    #[test]
    fn mute_queues_nothing_and_volumes_scale() {
        let mut q = SoundQueue::default();
        let muted = AudioSettings {
            mute: true,
            ..Default::default()
        };
        assert!(!q.push(Sound::RoadBuilt, 0.0, &muted));
        assert!(!q.push(
            Sound::RoadBuilt,
            1.0,
            &AudioSettings {
                master: 0.0,
                ..Default::default()
            }
        ));
        assert!(q.is_empty());
        let s = AudioSettings {
            master: 0.5,
            effects: 0.6,
            mute: false,
        };
        assert!((s.gain() - 0.3).abs() < 1e-6);
        assert!(q.push(Sound::RoadBuilt, 2.0, &s));
        assert_eq!(q.take().len(), 1);
        assert_eq!(AudioSettings::default().effects, 0.6);
    }

    #[test]
    fn the_same_sound_is_limited_to_ten_a_second_with_a_little_pitch_spread() {
        let s = AudioSettings::default();
        let mut q = SoundQueue::default();
        let mut played = 0;
        // 100 clicks in one second, a hundredth apart.
        for k in 0..100 {
            if q.push(Sound::ZonePainted, k as f64 * 0.01, &s) {
                played += 1;
            }
        }
        assert_eq!(played, 10);
        // Another sound is not held back by it.
        assert!(q.push(Sound::RoadBuilt, 0.5, &s));
        for (_, pitch) in q.take() {
            assert!((pitch - 1.0).abs() <= PITCH_SPREAD + 1e-6, "{pitch}");
        }
    }
}
