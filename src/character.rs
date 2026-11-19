//! Alice's intact skin, locomotion animation and a collision-aware following camera.
pub use crate::weapons::WeaponInput;
use crate::{
    assets::Assets,
    collision::World,
    movement::Player,
    skeletal::{Animation, Definition, Skeleton, Transform},
    texture,
    weapons::{Action, Actions, Visuals},
};
use anyhow::{Context, Result};
use macroquad::{
    miniquad::{Comparison, PipelineParams, ShaderSource},
    prelude::*,
};
use std::collections::BTreeMap;

pub(crate) const CLIPS: [&str; 36] = [
    "idle_base_01",
    "walk_nowep",
    "run_nowep",
    "jump_small_takeoff",
    "jump_air_03",
    "jump_small_land",
    "ready",
    "walk_smallwep",
    "run_smallwep",
    "walk_bigwep",
    "run_bigwep",
    "swim_frog",
    "swim_tred_wep",
    "ledge_climb_32",
    "ledge_climb_48",
    "ledge_climb_64",
    "death_faint",
    "death_falling_01",
    "death_drown",
    "rope_hang",
    "sit_minecart",
    "sit_airship",
    "rope_climb_up_right",
    "rope_climb_down_left",
    "ready_bw",
    "swim_tred",
    "ledge_hang",
    "ledge_shim_left",
    "ledge_shim_right",
    "ledge_pullup",
    "rope_climb_up_left",
    "rope_climb_down_right",
    // Ground deaths beyond death_faint (16), chosen by `death_clip`.
    "death_hideous",
    "death_forward",
    "death_hard",
    "death_knockback",
];
const DEATH_FAINT: usize = 16;
const DEATH_HIDEOUS: usize = 32;
const DEATH_FORWARD: usize = 33;
const DEATH_HARD: usize = 34;
const DEATH_KNOCKBACK: usize = 35;

/// The original `global/alice_torso.st` DEATH state: the first entry whose
/// conditions hold (each CHANCE rolled in order) picks Alice's ground death.
/// `damage` is the killing blow; `from_front` is None when it had no direction.
fn choose_death(damage: f32, from_front: Option<bool>, mut roll: impl FnMut() -> f32) -> usize {
    let front = from_front == Some(true);
    let rear = from_front == Some(false);
    if damage >= 40. && roll() < 0.7 {
        return DEATH_HIDEOUS;
    }
    if rear && roll() < 0.9 {
        return DEATH_FORWARD;
    }
    if front && damage >= 23. && roll() < 0.8 {
        return DEATH_HARD;
    }
    if front && roll() < 0.8 {
        return DEATH_KNOCKBACK;
    }
    if roll() < 0.6 {
        return DEATH_FAINT;
    }
    if roll() < 0.8 {
        return DEATH_HARD;
    }
    DEATH_HIDEOUS
}

fn rope_pair(index: usize) -> Option<usize> {
    match index {
        22 => Some(30),
        23 => Some(31),
        _ => None,
    }
}
fn rope_length(player: &Player) -> Option<f32> {
    (player.script_motion == 1)
        .then(|| {
            player
                .rope
                .as_ref()
                .map(|g| g.length)
                .or(player.script_rope_length)
        })
        .flatten()
}
/// The controller loads frame files, but sound commands belong to TIKI aliases.
pub(crate) fn sound_alias(clip: &str) -> &str {
    match clip {
        "jump_air_03" => "fall",
        "swim_frog" => "swim_forward_frog",
        "ledge_climb_32" => "climb_32",
        "ledge_climb_48" => "climb_48",
        "ledge_climb_64" => "climb_64",
        "death_falling_01" => "death_falling1",
        "death_drown" => "death_drown1",
        "ready_bw" => "ready_bigwep",
        "ledge_hang" => "hang_idle",
        "ledge_shim_left" => "shimmy_left",
        "ledge_shim_right" => "shimmy_right",
        _ => clip,
    }
}
#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
enum Motion {
    Idle,
    Walk,
    Run,
    Takeoff,
    Air,
    Land,
    Swim,
    Tread,
    Climb,
    Death,
    Rope,
    Cart,
    Airship,
    Hang,
    PullUp,
}
impl Motion {
    fn index(self) -> usize {
        self as usize
    }
    fn looping(self) -> bool {
        matches!(
            self,
            Self::Idle
                | Self::Walk
                | Self::Run
                | Self::Air
                | Self::Swim
                | Self::Tread
                | Self::Rope
                | Self::Cart
                | Self::Airship
                | Self::Hang
        )
    }
}

struct Animator {
    last_feet: Option<Vec3>,
    last_rope_length: Option<f32>,
    scale: f32,
    sound_clip: Option<usize>,
    resume_audio: bool,
    sound_span: Option<(usize, crate::audio::events::Span)>,
    clips: Vec<Animation>,
    motion: Motion,
    time: f32,
    transition: f32,
    previous: Vec<Transform>,
    pose: Vec<Transform>,
    jumps: u64,
    landings: u64,
    large: bool,
    unarmed: bool,
    offset: f32,
    previous_offset: f32,
    death_clip: usize,
}
impl Animator {
    fn snapshot(&self) -> AnimatorSave {
        AnimatorSave {
            last_feet: self.last_feet,
            last_rope_length: self.last_rope_length,
            motion: self.motion,
            time: self.time,
            transition: self.transition,
            previous: self.previous.clone(),
            pose: self.pose.clone(),
            jumps: self.jumps,
            landings: self.landings,
            large: self.large,
            offset: self.offset,
            previous_offset: self.previous_offset,
            death_clip: self.death_clip,
        }
    }
    fn restore(&mut self, s: &AnimatorSave) {
        self.death_clip = if s.death_clip < CLIPS.len() { s.death_clip } else { DEATH_FAINT };
        self.last_feet = s.last_feet;
        self.last_rope_length = s.last_rope_length;
        self.sound_span = None;
        self.sound_clip = None;
        self.resume_audio = true;
        self.motion = s.motion;
        self.time = s.time;
        self.transition = s.transition;
        self.previous.clone_from(&s.previous);
        self.pose.clone_from(&s.pose);
        self.jumps = s.jumps;
        self.landings = s.landings;
        self.large = s.large;
        self.offset = s.offset;
        self.previous_offset = s.previous_offset;
    }
    fn new(clips: Vec<Animation>) -> Self {
        let pose = clips[6].sample(0., true);
        Self {
            last_feet: None,
            last_rope_length: None,
            scale: 1.,
            sound_clip: None,
            resume_audio: false,
            sound_span: None,
            clips,
            motion: Motion::Idle,
            time: 0.,
            transition: 1.,
            previous: pose.clone(),
            pose,
            jumps: 0,
            landings: 0,
            large: false,
            unarmed: false,
            offset: 0.,
            previous_offset: 0.,
            death_clip: DEATH_FAINT,
        }
    }
    fn reset(&mut self, player: &Player) {
        self.last_feet = Some(player.feet);
        self.last_rope_length = rope_length(player);
        self.sound_clip = None;
        self.resume_audio = false;
        self.sound_span = None;
        self.motion = Motion::Idle;
        self.time = 0.;
        self.transition = 1.;
        self.jumps = player.jumps;
        self.landings = player.landings;
        self.pose = self.clips[6].sample(0., true);
        self.previous = self.pose.clone();
        self.offset = 0.;
        self.previous_offset = 0.;
    }
    fn update(&mut self, dt: f32, player: &Player, running: bool) {
        self.sound_span = None;
        if dt <= 0. {
            return;
        }
        let previous_feet = self.last_feet.replace(player.feet);
        let length = rope_length(player);
        let rope_travel = length
            .zip(self.last_rope_length)
            .map_or(0., |(now, before)| (now - before).abs());
        self.last_rope_length = length;
        let displacement = previous_feet.map_or(player.velocity * dt, |p| player.feet - p);
        // Warps reset gait; authored root motion must never pull a collision body.
        let displacement = if displacement.length() <= 128. {
            displacement
        } else {
            Vec3::ZERO
        };
        // A lift can carry the body while Alice stands still. Only her own
        // controller velocity selects a gait; resolved travel still sets its pace.
        let speed = if player.velocity.truncate().length() > 1. {
            displacement.truncate().length() / dt
        } else {
            0.
        };
        let old_motion = self.motion;
        let old_clip = self.sound_clip;
        let old_time = self.time;
        let jump = player.jumps != self.jumps;
        let land = player.landings != self.landings;
        self.jumps = player.jumps;
        self.landings = player.landings;
        let next = if self.motion == Motion::Death {
            Motion::Death
        } else if player.script_motion > 0 {
            match player.script_motion {
                1 => Motion::Rope,
                2 => Motion::Cart,
                _ => Motion::Airship,
            }
        } else if let Some(hang) = &player.ledge {
            if hang.pulling {
                Motion::PullUp
            } else {
                Motion::Hang
            }
        } else if player.climbing() {
            Motion::Climb
        } else if player.swimming {
            if player.velocity.length() > 20. {
                Motion::Swim
            } else {
                Motion::Tread
            }
        } else if !player.grounded {
            if jump
                || (self.motion == Motion::Takeoff && self.time < self.clips[3].duration() - 0.05)
            {
                Motion::Takeoff
            } else {
                Motion::Air
            }
        } else if speed > 1.
            || (dt < crate::movement::FIXED_DT
                && player.velocity.truncate().length() > 1.
                && matches!(self.motion, Motion::Walk | Motion::Run))
        {
            if running {
                Motion::Run
            } else {
                Motion::Walk
            }
        } else if land || (self.motion == Motion::Land && self.time < 0.4) {
            Motion::Land
        } else {
            Motion::Idle
        };
        if next != self.motion || jump || land {
            self.previous.clone_from(&self.pose);
            self.previous_offset = self.offset;
            self.transition = 0.;
            self.motion = next;
            // The controller jumps immediately; skip the original pre-jump anticipation.
            self.time = if next == Motion::Takeoff { 0.25 } else { 0. };
        }
        let index = match self.motion {
            Motion::Hang | Motion::PullUp => player.ledge.as_ref().map_or(26, |h| h.clip()),
            Motion::Death => {
                // alice_torso.st KILLED: DEATH_WATER needs IN_WATER "2" and !ON_FIRE.
                // Burning in lava is ON_FIRE: the ordinary DEATH table, not a fall.
                let lava = player.immersion.level > 0 && player.immersion.kind == crate::water::Liquid::Lava;
                if player.immersion.level >= 2 && !lava {
                    18
                } else if !player.grounded && !lava {
                    17
                } else {
                    self.death_clip
                }
            }
            Motion::Rope => match player
                .rope
                .as_ref()
                .map_or(player.script_rope_rise, |g| g.rise)
            {
                n if n > 0.001 => 22,
                n if n < -0.001 => 23,
                _ => 19,
            },
            Motion::Cart => 20,
            Motion::Airship => 21,
            Motion::Swim => 11,
            Motion::Tread => {
                if self.unarmed {
                    25
                } else {
                    12
                }
            }
            Motion::Climb => {
                if player.climb_height <= 34. {
                    13
                } else if player.climb_height <= 50. {
                    14
                } else {
                    15
                }
            }
            Motion::Idle => {
                if self.unarmed {
                    0
                } else if self.large {
                    24
                } else {
                    6
                }
            }
            Motion::Walk => {
                if self.unarmed {
                    1
                } else if self.large {
                    9
                } else {
                    7
                }
            }
            Motion::Run => {
                if self.unarmed {
                    2
                } else if self.large {
                    10
                } else {
                    8
                }
            }
            _ => self.motion.index(),
        };
        let entered = self.sound_clip != Some(index) && !self.resume_audio;
        if entered && !matches!(self.motion, Motion::Takeoff) {
            self.previous.clone_from(&self.pose);
            self.previous_offset = self.offset;
            self.transition = 0.;
            self.time = 0.;
        }
        self.sound_clip = Some(index);
        self.resume_audio = false;
        let clip = &self.clips[index];
        let duration = self.duration(index);
        let preserve_stride = matches!(old_motion, Motion::Walk | Motion::Run)
            && matches!(self.motion, Motion::Walk | Motion::Run);
        if preserve_stride && old_clip.is_some_and(|old| old != index) {
            self.time = old_time / self.clips[old_clip.unwrap()].duration() * clip.duration();
        }
        let end = if let Some(hang) = &player.ledge {
            if hang.elapsed + 0.001 < self.time {
                self.time -= clip.duration();
            }
            hang.elapsed
        } else if matches!(self.motion, Motion::Walk | Motion::Run) {
            clip.advance_distance(
                self.time,
                displacement.truncate().length() / self.scale,
                false,
            )
        } else if self.motion == Motion::Rope && matches!(index, 22 | 23) {
            // A climb stroke includes stationary hand-placement frames. Inverting
            // each frame's root delta skips these poses and snaps the limbs. Pace
            // the complete alternating-hand cycle by actual travel along the rope.
            let travel: f32 = [index, rope_pair(index).unwrap()]
                .into_iter()
                .flat_map(|i| &self.clips[i].frames)
                .map(|f| f.delta.z.abs())
                .sum();
            self.time + rope_travel / self.scale / travel.max(0.001) * duration
        } else if let Some(progress) = player
            .climb_progress()
            .filter(|_| self.motion == Motion::Climb)
        {
            progress * clip.duration()
        } else {
            self.time + dt
        };
        self.sound_span = Some((
            index,
            crate::audio::events::Span {
                start: self.time,
                end,
                duration,
                frame_time: clip.frame_time,
                looping: self.motion.looping(),
                entered: entered && !preserve_stride,
            },
        ));
        self.time = end;
        if self.motion.looping() {
            self.time = self.time.rem_euclid(duration);
        } else {
            self.time = self.time.min(clip.duration());
        }
        self.transition = (self.transition + dt / 0.12).min(1.);
        let sample = self.sample(index);
        let t = self.transition * self.transition * (3. - 2. * self.transition);
        // The frog stroke is authored ~24 units lower than the upright tread pose.
        // Blend its presentation origin with the pose, not the collision body.
        let offset = if self.motion == Motion::Swim { 24. } else { 0. };
        self.offset = self.previous_offset + (offset - self.previous_offset) * t;
        self.pose = self
            .previous
            .iter()
            .zip(sample)
            .map(|(&a, b)| a.blend(b, t))
            .collect();
    }

    fn duration(&self, index: usize) -> f32 {
        self.clips[index].duration() + rope_pair(index).map_or(0., |i| self.clips[i].duration())
    }

    fn sample(&self, index: usize) -> Vec<Transform> {
        let Some(other) = rope_pair(index) else {
            return self.clips[index].sample(self.time, self.motion.looping());
        };
        let (current, next, time) = if self.time < self.clips[index].duration() {
            (index, other, self.time)
        } else {
            (other, index, self.time - self.clips[index].duration())
        };
        let clip = &self.clips[current];
        let last = clip.duration() - clip.frame_time;
        if time < last {
            clip.sample(time, false)
        } else {
            clip.frames
                .last()
                .unwrap()
                .pose
                .iter()
                .zip(&self.clips[next].frames[0].pose)
                .map(|(&a, &b)| a.blend(b, ((time - last) / clip.frame_time).clamp(0., 1.)))
                .collect()
        }
    }

    fn audio_spans(
        &self,
        index: usize,
        span: crate::audio::events::Span,
    ) -> Vec<(usize, crate::audio::events::Span)> {
        let Some(other) = rope_pair(index) else {
            return vec![(index, span)];
        };
        let mut spans = Vec::new();
        let duration = self.duration(index);
        let mut start = span.start;
        // Split at each handoff so both original hand cues keep their own frames.
        while start < span.end && spans.len() < 32 {
            let cycle = (start / duration).floor() * duration;
            let (i, offset) = if start - cycle < self.clips[index].duration() {
                (index, cycle)
            } else {
                (other, cycle + self.clips[index].duration())
            };
            let end = span.end.min(offset + self.clips[i].duration());
            if end <= start {
                break;
            }
            spans.push((
                i,
                crate::audio::events::Span {
                    start: start - offset,
                    end: end - offset,
                    duration: self.clips[i].duration(),
                    frame_time: self.clips[i].frame_time,
                    looping: false,
                    entered: (span.entered && start == span.start) || start > span.start,
                },
            ));
            start = end;
        }
        spans
    }
}

pub struct Character {
    pub weapon_notice: Option<&'static str>,
    _material_layers: Vec<std::rc::Rc<crate::render_fx::Surface>>,
    foot_rig: crate::footing::Rig,
    foot_state: crate::footing::State,
    acting: crate::acting::Acting,
    face: crate::facial::Face,
    mouth: f32,
    face_time: f32,
    sound_events: crate::audio::events::Model,
    power_pose: crate::power_pose::Performance,
    power_art: crate::power_pose::Art,
    water_art: crate::water_art::Art,
    skeleton: Skeleton,
    animator: Animator,
    meshes: Vec<Option<Mesh>>,
    material: SkinMaterial,
    scale: f32,
    facing: f32,
    actions: Actions,
    visuals: Visuals,
    upper: Vec<bool>,
    /// models/alice.tik `burn` (torso, ON_FIRE in alice_torso.st STAND): arms flail
    /// while she burns; its entry plays burn_scream.wav.
    burn: Burn,
    /// Optional off-hand grip for a long Mallet mesh (data pack marker twohand.txt).
    two_hand: Option<TwoHand>,
    tags: [usize; 3],
    anchors: [Transform; 3],
    viewmodel: crate::viewmodel::ViewModel,
}
impl Character {
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            power: self.power_pose.state.clone(),
            feet: self.foot_state.clone(),
            acting: self.acting.state.clone(),
            face_time: self.face_time,
            animator: self.animator.snapshot(),
            facing: self.facing,
            actions: self.actions.snapshot(),
            projectiles: self.visuals.snapshot(),
            anchors: self.anchors,
            viewmodel: self.viewmodel.clone(),
        }
    }
    pub fn restore(&mut self, s: &Snapshot) -> Result<()> {
        self.apply_snapshot(s, false)
    }
    /// Live snapshots retain particle caches; disk restores reset presentation.
    pub fn replicate(&mut self, s: &Snapshot) -> Result<()> {
        self.apply_snapshot(s, true)
    }
    fn apply_snapshot(&mut self, s: &Snapshot, replica: bool) -> Result<()> {
        s.validate(self.skeleton.bones.len())?;
        self.power_pose.state = s.power.clone();
        self.foot_state = s.feet.clone();
        self.actions.restore(&s.actions)?;
        self.acting.restore(&s.acting)?;
        if replica { self.visuals.replicate(&s.projectiles)?; }
        else { self.visuals.restore(&s.projectiles)?; }
        self.animator.restore(&s.animator);
        self.face_time = s.face_time;
        self.mouth = 0.;
        self.facing = s.facing;
        self.anchors = s.anchors;
        self.viewmodel = s.viewmodel.clone();
        if !replica { self.water_art.reset(); }
        Ok(())
    }
    pub fn die(&mut self) {
        self.die_from(0., Vec3::ZERO);
    }
    /// `impulse` is the killing blow's knockback (pointing away from the attacker).
    pub fn die_from(&mut self, damage: f32, impulse: Vec3) {
        if !matches!(self.animator.motion, Motion::Death) {
            let flat = impulse.truncate();
            let from_front = (flat.length_squared() > 1e-4).then(|| {
                let forward = Vec2::new(self.facing.cos(), self.facing.sin());
                flat.normalize().dot(forward) < 0.
            });
            self.animator.death_clip =
                choose_death(damage, from_front, || macroquad::rand::gen_range(0., 1.));
        }
        self.power_pose.state.sync(None);
        if !matches!(
            self.animator.motion,
            Motion::Death | Motion::Rope | Motion::Cart | Motion::Airship
        ) {
            self.animator.previous.clone_from(&self.animator.pose);
            self.animator.previous_offset = self.animator.offset;
            self.animator.motion = Motion::Death;
            self.animator.transition = 0.;
            self.animator.time = 0.;
            self.actions.reset(&self.animator.pose);
            self.visuals.cancel_staff();
            self.visuals.clear();
        }
    }
    pub fn load(assets: &mut Assets) -> Result<Self> {
        Self::load_for_player(assets, crate::coop::PlayerId::Blue)
    }
    pub fn load_for_player(assets: &mut Assets, player: crate::coop::PlayerId) -> Result<Self> {
        let def = Definition::alice(assets)?;
        let skeleton = Skeleton::parse(&assets.read(&format!("{}/{}", def.path, def.model))?)?;
        let bone_count = skeleton.bones.len();
        let mut clips = Vec::new();
        for name in CLIPS {
            let path = format!("{}/{name}.ska", def.path);
            clips.push(
                Animation::parse(&assets.read(&path)?, skeleton.bones.len())
                    .with_context(|| path)?,
            );
        }
        let specs = texture::read_materials(assets)?;
        let mut textures = BTreeMap::<String, Texture2D>::new();
        let mut meshes = Vec::new();
        let mut material_layers = Vec::new();
        for surface in &skeleton.surfaces {
            // The extra cap surfaces are only used by the original dismemberment system.
            if ["material15", "material16", "material17"].contains(&surface.name.as_str()) {
                meshes.push(None);
                continue;
            }
            let skin = def
                .skins
                .get(&surface.name)
                .context("Alice surface has no skin")?;
            let path = texture::resolve(assets, &format!("{}/{skin}", def.path), &specs)
                .or_else(|| texture::resolve(assets, skin, &specs))
                .with_context(|| format!("Alice skin not found: {skin}"))?;
            let tex = if let Some(tex) = textures.get(&path) {
                tex.clone()
            } else {
                let mut image = texture::decode(assets, &path)?;
                if player == crate::coop::PlayerId::Red {
                    crate::coop::palette::red_clothing(&path, &mut image.pixels);
                }
                let tex = Texture2D::from_rgba8(image.width, image.height, &image.pixels);
                tex.set_filter(FilterMode::Linear);
                textures.insert(path, tex.clone());
                tex
            };
            material_layers.push(crate::render_fx::register(
                assets,
                &tex,
                &[format!("{}/{skin}", def.path), skin.clone()],
                &specs,
            )?);
            meshes.push(Some(Mesh {
                vertices: surface
                    .vertices
                    .iter()
                    .map(|v| Vertex {
                        position: Vec3::ZERO,
                        uv: v.uv,
                        color: [255; 4],
                        normal: Vec4::ZERO,
                    })
                    .collect(),
                indices: surface.indices.clone(),
                texture: Some(tex),
            }));
        }
        let face = crate::facial::Face::load(
            assets,
            "models/alice.tik",
            &def,
            &skeleton,
            &meshes,
            &specs,
        )?;
        let material = skin_material()?;
        println!(
            "Alice: {} bones, {} visible surfaces, {} locomotion clips",
            skeleton.bones.len(),
            meshes.iter().flatten().count(),
            clips.len()
        );
        let tag = |name| {
            skeleton
                .bones
                .iter()
                .position(|b| b.name == name)
                .with_context(|| format!("Missing Alice attachment {name}"))
        };
        let two_hand = TwoHand::load(assets, &skeleton);
        let tags = [
            tag("tag_weapon")?,
            tag("tag_ball_linked")?,
            tag("tag_ball_free")?,
        ];
        let power_pose = crate::power_pose::Performance::load(assets, &def, skeleton.bones.len())?;
        let power_art = crate::power_pose::Art::load(assets, &skeleton, &meshes, &specs)?;
        let water_art = crate::water_art::Art::load(assets, &skeleton, &specs)?;
        let mut upper = Vec::new();
        for b in &skeleton.bones {
            upper.push(b.name == "Bip01 Spine" || b.parent.is_some_and(|p| upper[p]));
        }
        let mut animator = Animator::new(clips);
        animator.scale = def.scale;
        let foot_rig = crate::footing::Rig::new(&skeleton, &animator.pose, def.scale);
        let actions = Actions::load(assets, skeleton.bones.len(), &animator.pose)?;
        let visuals = Visuals::load(assets)?;
        Ok(Self {
            weapon_notice: None,
            foot_rig,
            _material_layers: material_layers,
            foot_state: Default::default(),
            acting: crate::acting::Acting::load(assets, &def, &skeleton, &specs)?,
            face,
            mouth: 0.,
            face_time: 0.,
            sound_events: crate::audio::events::Model::load(assets, "models/alice.tik")?,
            power_pose,
            power_art,
            water_art,
            skeleton,
            animator,
            meshes,
            material,
            scale: def.scale,
            facing: 0.,
            actions,
            visuals,
            upper,
            burn: Burn {
                clip: Animation::parse(&assets.read(&format!("{}/burn.ska", def.path))?, bone_count)
                    .context("Alice burn.ska")?,
                time: 0.,
                weight: 0.,
                scream: false,
                was_burning: false,
            },
            two_hand,
            tags,
            anchors: [Transform {
                rotation: Quat::IDENTITY,
                translation: Vec3::ZERO,
            }; 3],
            viewmodel: crate::viewmodel::ViewModel::default(),
        })
    }
    pub fn reset(&mut self, player: &Player, yaw: f32) {
        self.water_art.reset();
        self.water_art.sync(player);
        self.power_pose.state.interrupt();
        self.foot_state = Default::default();
        self.acting.reset();
        self.face_time = 0.;
        self.mouth = 0.;
        self.facing = yaw;
        self.animator.reset(player);
        self.actions.reset(&self.animator.pose);
        self.visuals.clear();
        self.viewmodel.reset();
    }
    pub fn notarget(&mut self, value: bool) {
        self.visuals.dice.notarget = value;
    }
    pub fn facing(&self) -> f32 {
        self.facing
    }
    pub fn rage_scene(&mut self, time: f32, finished: bool, player: &Player) {
        self.power_pose
            .rage_scene(time, finished, &mut self.actions.pose);
        if !finished {
            self.facing = player.script_facing;
            self.acting.reset();
            self.foot_state = Default::default();
            self.visuals.cancel_staff();
        }
    }
    /// Resume from the actual last cinematic pose without clearing live effects.
    /// A distant cutaway or a scaled Alice cannot seed an ordinary body pose.
    pub fn resume_scene(
        &mut self,
        player: &Player,
        yaw: f32,
        pose: Option<&crate::cinematic::ActorPose>,
    ) {
        self.power_pose.state.interrupt();
        self.foot_state = Default::default();
        self.acting.reset();
        self.mouth = 0.;
        self.facing = yaw;
        self.animator.reset(player);
        if let Some(pose) = pose.filter(|p| {
            p.local.len() == self.skeleton.bones.len()
                && (p.scale - 1.).abs() < 0.01
                && p.transform.translation.distance(player.feet) <= 16.
        }) {
            self.facing = pose.transform.rotation.to_euler(EulerRot::ZYX).0;
            self.animator.pose.clone_from(&pose.local);
            self.animator.previous.clone_from(&pose.local);
            self.animator.transition = 0.;
            self.animator.resume_audio = true;
        }
        self.actions.reset(&self.animator.pose);
    }
    pub fn cancel_weapon_action(&mut self) {
        self.actions.reset(&self.animator.pose);
        self.visuals.cancel_staff();
        self.weapon_notice = None;
    }
    pub fn update(
        &mut self,
        dt: f32,
        player: &Player,
        running: bool,
        weapon: WeaponInput,
        context: &crate::combat::Context<'_>,
    ) -> Vec<&'static str> {
        if !dt.is_finite() || dt <= 0. {
            self.animator.sound_span = None;
            return Vec::new();
        }
        self.update_funded(dt, player, running, weapon, context, None)
    }
    pub fn update_funded(
        &mut self,
        dt: f32,
        player: &Player,
        running: bool,
        mut weapon: WeaponInput,
        context: &crate::combat::Context<'_>,
        mut stats: Option<&mut crate::inventory::Stats>,
    ) -> Vec<&'static str> {
        self.water_art.sync(player);
        if let Some(stats) = stats.as_deref() {
            self.power_appearance(stats);
        }
        if !dt.is_finite() || dt <= 0. {
            self.animator.sound_span = None;
            return Vec::new();
        }
        self.water_art.burn_step(dt, player);
        self.visuals.dice_count = weapon.dice.min(3);
        if weapon.selected == 6 && !self.visuals.dice.ready() {
            weapon.click = None;
        }
        // A long Mallet mesh (data pack with twohand.txt) uses the authored two-handed stance of
        // the other large weapons while idle and walking.
        self.animator.large = [4, 7, 8].contains(&weapon.selected)
            || (weapon.selected == 2 && self.two_hand.is_some());
        // The hobby horse's alternate is a free melee jump slam, not a croquet ball shot.
        self.actions.free_alternate = self.two_hand.is_some();
        self.visuals.hobby_horse = self.two_hand.is_some();
        self.animator.unarmed = weapon.selected == crate::weapons::UNARMED;
        self.animator.update(
            if self.visuals.ice_locked() { 0. } else { dt },
            player,
            running,
        );
        if self.animator.motion == Motion::Death
            || player.script_motion > 0
            || player.ledge.is_some()
        {
            self.power_pose
                .update(dt, false, false, &mut self.actions.pose, &self.upper);
            self.acting.update(dt, false, false, false, weapon.selected);
            self.actions.reset(&self.animator.pose);
            self.visuals.cancel_staff();
            if player.script_motion > 0 {
                self.facing = player.script_facing;
            }
            if let Some(hang) = &player.ledge {
                self.facing = hang.forward.y.atan2(hang.forward.x);
                let pose = self.skeleton.global_pose(&self.actions.pose);
                let rotation = Quat::from_rotation_z(self.facing);
                self.anchors = self.tags.map(|i| Transform {
                    rotation: rotation * pose[i].rotation,
                    translation: player.feet + rotation * (pose[i].translation * self.scale),
                });
            }
            if self.animator.motion == Motion::Rope || player.ledge.is_some() {
                // Hanging suppresses new attacks, not already released projectiles
                // or a summoned demon. Their ordinary pause gate still applies.
                return self.visuals.update(
                    dt,
                    &self.actions,
                    Default::default(),
                    &self.anchors,
                    player.eye(),
                    context,
                );
            }
            return Vec::new();
        }
        let attack_aim = self
            .actions
            .playing
            .as_ref()
            .filter(|p| matches!(p.action, Action::Attack { .. }))
            .map(|_| weapon.aim)
            .or_else(|| {
                (weapon.click.is_some()
                    && crate::weapons::supported(weapon.selected)
                    && self.actions.playing.is_none())
                .then_some(weapon.aim)
            });
        if dt > 0. && player.climbing() {
            self.facing = player.climb_direction.y.atan2(player.climb_direction.x);
        } else if dt > 0. && attack_aim.is_some() {
            let aim = attack_aim.unwrap();
            self.facing = aim.y.atan2(aim.x);
        } else if dt > 0. && player.velocity.truncate().length() > 12. {
            let target = player.velocity.y.atan2(player.velocity.x);
            let difference = (target - self.facing + std::f32::consts::PI)
                .rem_euclid(std::f32::consts::TAU)
                - std::f32::consts::PI;
            self.facing = (self.facing + difference.clamp(-dt * 12., dt * 12.))
                .rem_euclid(std::f32::consts::TAU);
        }
        if self.visuals.ice_locked() {
            weapon.click = None;
        }
        self.visuals.owner_immersed =
            player.immersion.level > 1 && player.immersion.kind != crate::water::Liquid::Lava;
        self.visuals.follow_ice(player.feet);
        let first_person = weapon.first_person;
        let selected = weapon.selected;
        if selected != 4
            || (weapon.click != Some(false)
                && self.actions.playing.as_ref().is_none_or(|p| p.clip != 13))
        {
            self.visuals.stop_ice();
        }
        let aim = weapon.aim;
        let mut events = self.actions.update_funded(
            dt,
            weapon,
            &self.animator.pose,
            &self.upper,
            player.grounded && player.velocity.truncate().length() < 12.,
            stats.as_deref_mut(),
        );
        self.weapon_notice = events.notice;
        let idle = self.animator.motion == Motion::Idle;
        self.acting.update(
            dt,
            idle,
            player.grounded && !player.climbing() && !first_person,
            self.actions.playing.is_some(),
            selected,
        );
        if dt > 0. || self.animator.unarmed {
            self.acting.apply(&mut self.actions.pose, &self.upper, idle);
        }
        self.burn.apply(
            dt,
            self.water_art.burning() && !player.swimming,
            &mut self.actions.pose,
            &self.upper,
        );
        let mut power_mask = self.upper.clone();
        if let Some(i) = self.skeleton.bones.iter().position(|b| b.name == "tag_03") {
            power_mask[i] = true;
        }
        self.power_pose.update(
            dt,
            self.actions.playing.is_none()
                && player.grounded
                && !player.climbing()
                && !player.swimming,
            idle,
            &mut self.actions.pose,
            &power_mask,
        );
        if let Some(grip) = &mut self.two_hand {
            // Long Mallet mesh: both arms follow the procedural carry and overhead slam.
            let held = self.actions.equipment().0 == 2
                && self.actions.equipment().1 > 0.5
                && !first_person
                && !player.swimming
                && !player.climbing();
            let play = self
                .actions
                .playing
                .as_ref()
                .filter(|p| matches!(p.action, Action::Attack { .. }))
                .map(|p| {
                    let alt = p.alternate();
                    let swing = if alt {
                        HhSwing::Combo
                    } else if crate::weapons::ACTION_CLIPS[p.clip] == "mallet_att_3_prim_02" {
                        HhSwing::Spin
                    } else {
                        HhSwing::Slam
                    };
                    (p.time, p.duration, crate::weapon_rules::rule(2, alt, 0).release, swing)
                });
            grip.apply(dt, held, play, &self.skeleton, self.tags[0], &mut self.actions.pose);
        }
        self.foot_rig.apply(
            &self.skeleton,
            &mut self.actions.pose,
            &mut self.foot_state,
            context.world,
            Transform {
                translation: player.feet,
                rotation: Quat::from_rotation_z(self.facing),
            },
            self.scale,
            dt,
            player.grounded
                && !player.climbing()
                && !player.swimming
                && !first_person
                && matches!(
                    self.animator.motion,
                    Motion::Idle | Motion::Walk | Motion::Run | Motion::Land
                ),
        );
        let pose = self.skeleton.global_pose(&self.actions.pose);
        let rotation = Quat::from_rotation_z(self.facing);
        self.anchors = self.tags.map(|i| Transform {
            rotation: rotation * pose[i].rotation,
            translation: player.feet
                + Vec3::Z * self.animator.offset
                + rotation * (pose[i].translation * self.scale),
        });
        self.visuals.set_combat_anchors(self.anchors, aim);
        events.attach_contacts(|local| {
            let pose = self.skeleton.global_pose(local);
            self.tags.map(|i| Transform {
                rotation: rotation * pose[i].rotation,
                translation: player.feet
                    + Vec3::Z * self.animator.offset
                    + rotation * (pose[i].translation * self.scale),
            })
        });
        let mut view_scale = 1.;
        if first_person {
            let view = self.viewmodel.pose(
                dt,
                player.velocity.length(),
                &self.actions,
                crate::viewmodel::camera_frame(player.eye(), aim),
            );
            self.anchors = view.anchors;
            view_scale = view.scale;
        }
        self.visuals.set_view(first_person, view_scale);
        self.visuals.owner_velocity = player.velocity;
        let sounds = self.visuals.update_funded(
            dt,
            &self.actions,
            events,
            &self.anchors,
            player.eye(),
            context,
            stats.as_deref_mut(),
        );
        if let Some(stats) = stats {
            if self.visuals.ice_drain {
                stats.spend_will(stats.will());
            } else if self.visuals.ice_refund > 0. {
                stats.apply(crate::inventory::PickupKind::Will, self.visuals.ice_refund);
            }
        }
        self.visuals.ice_drain = false;
        self.visuals.ice_refund = 0.;
        self.visuals.finish_staff(&mut self.actions);
        self.actions.finish_frame();
        sounds
    }
    pub fn ice_locked(&self) -> bool {
        self.visuals.ice_locked()
    }
    pub fn frozen(&self) -> Option<(f32, Vec3, f32)> {
        self.visuals.frozen()
    }
    /// A Phantasmagoria ice chain reached Alice: encase her in an fx_iceball block.
    pub fn phantasm_freeze(&mut self, feet: Vec3) -> bool {
        self.visuals.phantasm_freeze(feet, self.facing)
    }
    /// Alice is admiring the Darkened Looking Glass: the original locks control and
    /// closes the camera in on her until the clip ends.
    pub fn glass_age(&self) -> f32 {
        self.power_pose.state.age
    }
    pub fn glass_performing(&self) -> bool {
        self.power_pose.state.performing
            && self.power_pose.state.form == Some(crate::power_pose::Form::Glass)
    }
    pub fn attack_movement_locked(&self) -> bool {
        self.actions.movement_locked()
    }
    pub fn ice_targets(&self) -> Vec<crate::combat::Target> {
        self.visuals.ice_targets()
    }
    pub fn croquet_contacts(&self) -> bool {
        self.actions.equipment().0 == 2 || self.visuals.croquet_in_flight()
    }
    pub fn hit_ice(&mut self, hit: crate::combat::Hit) -> bool {
        self.visuals.hit_ice(hit)
    }
    pub fn audio_loops(&self) -> Vec<crate::audio::LoopCue> {
        self.visuals.audio_loops()
    }
    pub fn take_world_audio(&mut self) -> Vec<(&'static str, Vec3)> {
        std::mem::take(&mut self.visuals.spatial_sounds)
    }
    pub fn take_audio(&mut self) -> Vec<crate::audio::events::Cue> {
        let mut cues = self.acting.take_audio();
        if std::mem::take(&mut self.burn.scream) {
            // `entry sound sound/character/alice/burn_scream.wav 1 .7 384`
            cues.push(crate::audio::events::Cue {
                path: "sound/character/alice/burn_scream.wav".into(),
                volume: 1.,
            });
        }
        let Some((index, span)) = self.animator.sound_span.take() else {
            return cues;
        };
        for (index, span) in self.animator.audio_spans(index, span) {
            cues.extend(
                self.sound_events
                    .between(sound_alias(CLIPS[index]), span)
                    .into_iter()
                    .filter(|cue| {
                        // Immediate controller jump/landing contacts already own these cues.
                        !cue.path.ends_with("/jump.wav") && !cue.path.ends_with("/small_land.wav")
                    }),
            );
        }
        cues
    }
    pub fn ready_to_attack(&self, selected: usize) -> bool {
        self.actions.ready_to_attack(selected)
            && self.animator.motion != Motion::Death
            && (selected != 6 || self.visuals.dice.ready())
    }
    pub fn summon_target(&self) -> Option<crate::combat::Target> {
        self.visuals.dice.target()
    }
    pub fn ally_target(&self) -> Option<crate::combat::Target> {
        self.visuals.dice.ally_target()
    }
    pub fn dismiss_summon(&mut self) {
        self.visuals.dice.dismiss();
    }
    pub fn hit_summon(&mut self, hit: crate::combat::Hit) {
        self.visuals.dice.hit(hit);
    }
    pub fn threatens(&self, world: &World, target: crate::combat::Target) -> bool {
        self.visuals.threatens(world, target)
    }
    pub fn take_hits(&mut self) -> Vec<crate::combat::Hit> {
        std::mem::take(&mut self.visuals.hits)
    }
    pub fn atmosphere(&self, atmosphere: &crate::environment::Atmosphere, camera: Vec3) {
        self.water_art.atmosphere(atmosphere, camera);
        self.material.atmosphere(atmosphere, camera);
        self.visuals.atmosphere(atmosphere, camera);
        self.power_art.atmosphere(camera);
        self.power_art.atmosphere_full(atmosphere, camera);
    }
    pub fn world_time(&mut self, dt: f32, world_dt: f32) {
        let frozen = world_dt == 0. && dt > 0.;
        self.visuals.time_stopped = frozen;
        self.visuals.world_ratio = Some(if dt > 0. {
            (world_dt / dt).clamp(0., 1.)
        } else {
            0.
        });
        self.actions.time_stopped = frozen;
    }
    pub fn power_appearance(&mut self, stats: &crate::inventory::Stats) {
        self.power_art.remaining(stats.powers.rage);
        self.visuals.dice.rage = stats.powers.rage > 0.;
        self.visuals.dice.difficulty = stats.difficulty;
        if !stats.alive() {
            self.visuals.dice.dismiss();
        }
        self.acting.health(stats.sanity());
        self.power_pose.state.sync(power_form(stats));
        if stats.equipped().is_none() {
            self.actions.unarm(&self.animator.pose);
        }
    }
    pub fn water_appearance(&mut self, player: &Player) {
        self.water_art.sync(player);
    }
    pub fn story_pose(&mut self, story: &crate::story::Story, dt: f32) {
        self.acting.dialogue(story);
        self.face_time += dt.max(0.);
        self.mouth = if self.animator.motion == Motion::Death {
            0.
        } else {
            story.mouth(&["fakeplayer", "player", "alice"])
        };
    }
    pub fn draw(&mut self, feet: Vec3, fullbright: bool) {
        self.draw_ghost(feet, fullbright, false);
    }
    /// Interpolated feet must carry the held toy with them. World projectiles
    /// keep their authoritative positions; this offset lasts only for drawing.
    pub fn draw_replica(&mut self, feet: Vec3, simulated_feet: Vec3, fullbright: bool) {
        let anchors = self.anchors;
        for anchor in &mut self.anchors { anchor.translation += feet - simulated_feet; }
        self.draw_ghost(feet, fullbright, false);
        self.anchors = anchors;
    }
    /// Co-op ropes follow the raised supporting hand in the displayed pose,
    /// including the same feet interpolation and root offset as the skin.
    pub fn replica_rope_hand(&self, feet: Vec3) -> Option<Vec3> {
        if self.animator.motion != Motion::Rope { return None; }
        let pose = self.skeleton.global_pose(&self.actions.pose);
        let rotation = Quat::from_rotation_z(self.facing);
        [["Bip01 L Finger1", "Bip01 L Finger2"], ["Bip01 R Finger1", "Bip01 R Finger2"]]
            .into_iter().filter_map(|names| {
            let finger = |name| self.skeleton.bones.iter()
                .position(|b| b.name == name)
                .map(|i| pose[i].translation);
            let palm = (finger(names[0])? + finger(names[1])?) * 0.5;
            Some(feet + Vec3::Z * self.animator.offset + rotation * (palm * self.scale))
        }).max_by(|a, b| a.z.total_cmp(&b.z))
    }
    fn draw_body(&mut self, feet: Vec3, fullbright: bool, ghost: bool) {
        let feet = feet + Vec3::Z * self.animator.offset;
        let pose = self
            .face
            .pose(&self.skeleton, &self.actions.pose, self.mouth);
        self.power_art
            .skin(&mut self.meshes, &self.power_pose.state);
        if self.power_pose.state.skin().is_none() {
            self.face.blink(
                &mut self.meshes,
                self.face_time,
                self.animator.motion != Motion::Death,
            );
        }
        let mut hidden = Vec::new();
        for (i, surface) in self.skeleton.surfaces.iter().enumerate() {
            if self.power_pose.state.hidden(&surface.name) {
                hidden.push((i, self.meshes[i].take()));
            }
        }
        let rotation = Quat::from_rotation_z(self.facing);
        draw_skin(
            &self.skeleton,
            &mut self.meshes,
            &pose,
            Transform {
                translation: feet,
                rotation,
            },
            self.scale,
            fullbright,
        );
        self.visuals.frozen_body(&self.meshes);
        self.power_art.surface_effects(&self.meshes, &self.power_pose.state);
        for (i, mesh) in hidden {
            self.meshes[i] = mesh;
        }
        self.power_art.draw(
            &self.power_pose.state,
            &pose,
            Transform {
                translation: feet,
                rotation,
            },
            self.scale,
            fullbright,
            self.power_pose.duration(crate::power_pose::Form::Glass),
        );
        self.material.bind_appearance(Appearance {
            ghost,
            ..Default::default()
        });
        self.acting.draw(
            &self.skeleton,
            &pose,
            Transform {
                translation: feet,
                rotation,
            },
            self.scale,
            fullbright,
        );
        if self.animator.motion != Motion::Death {
            self.water_art.draw(
                &pose,
                Transform { translation: feet, rotation },
                self.scale,
                self.animator.time,
                fullbright,
            );
            self.material.bind_appearance(Appearance { ghost, ..Default::default() });
        }
        self.water_art.draw_fire(feet, self.scale);
        self.material.bind_appearance(Appearance { ghost, ..Default::default() });
        if !matches!(
            self.animator.motion,
            Motion::Death | Motion::Rope | Motion::Cart | Motion::Airship
        ) && !self.acting.hide_weapon()
            && !self.power_pose.state.performing
        {
            self.visuals
                .draw_held(&self.actions, &self.anchors, fullbright);
        }
        gl_use_default_material();
    }
    pub fn draw_ghost(&mut self, feet: Vec3, fullbright: bool, ghost: bool) {
        let material = self.material.clone();
        material.draw(
            Appearance {
                ghost,
                ..Default::default()
            },
            || self.draw_body(feet, fullbright, ghost),
        );
        gl_use_default_material();
    }
    pub fn draw_effects(&mut self, camera: Vec3, fullbright: bool, show_alice: bool) {
        self.visuals
            .draw_effects(&self.material, camera, fullbright, show_alice);
    }
    pub fn draw_first_person(&mut self, fullbright: bool) {
        self.material.bind();
        self.visuals
            .draw_held(&self.actions, &self.anchors, fullbright);
        crate::render::depth_read_only(|| self.visuals.draw_view_trail());
        gl_use_default_material();
    }
    pub fn visual_counts(&self) -> (u64, u64) {
        (self.visuals.shots, self.visuals.impacts)
    }
    pub fn lights(&self) -> Vec<crate::lighting::Light> {
        let mut lights = self.visuals.lights();
        lights.extend(self.power_art.glass_light(
            &self.power_pose.state,
            self.power_pose.duration(crate::power_pose::Form::Glass),
        ));
        lights
    }
    /// Renderer regression after a staged scene; keep the live shared pipeline.
    pub(crate) async fn check_visible(&mut self, label: &str) -> Result<()> {
        let player = crate::movement::Player::new(Vec3::ZERO);
        self.reset(&player, 0.);
        visibility_camera(Vec3::ZERO, 1.);
        self.atmosphere(&crate::environment::Atmosphere::default(), player.eye());
        let before = get_screen_data();
        self.draw_ghost(Vec3::ZERO, true, false);
        let pixels = visible_pixels(&before, &visibility_image());
        anyhow::ensure!(
            pixels > 200,
            "Gameplay Alice missing after {label}: {pixels} pixels"
        );
        gl_use_default_material();
        set_default_camera();
        next_frame().await;
        println!("PASS gameplay Alice after {label}: {pixels} pixels");
        Ok(())
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    power: crate::power_pose::State,
    #[serde(default)]
    feet: crate::footing::State,
    #[serde(default)]
    acting: crate::acting::State,
    #[serde(default)]
    face_time: f32,
    animator: AnimatorSave,
    facing: f32,
    actions: crate::weapons::Snapshot,
    projectiles: crate::weapons::ProjectileSave,
    anchors: [Transform; 3],
    viewmodel: crate::viewmodel::ViewModel,
}
/// The transformation Alice wears for these stats: none while dead or without a power-up.
pub fn power_form(stats: &crate::inventory::Stats) -> Option<crate::power_pose::Form> {
    if !stats.alive() {
        None
    } else if stats.powers.rage > 0. {
        Some(crate::power_pose::Form::Rage)
    } else if stats.powers.tea > 0. {
        Some(crate::power_pose::Form::Tea)
    } else if stats.invisible > 0. {
        Some(crate::power_pose::Form::Glass)
    } else {
        None
    }
}
impl Snapshot {
    /// The shape checks `Character::restore` applies before it accepts a saved block; they
    /// depend only on the rig's bone count.
    pub fn validate(&self, bones: usize) -> Result<()> {
        self.power.validate(bones)?;
        self.feet.validate()?;
        anyhow::ensure!(
            self.animator.pose.len() == bones
                && self.animator.previous.len() == bones
                && self.animator.last_feet.is_none_or(|p| p.is_finite())
                && self
                    .animator
                    .last_rope_length
                    .is_none_or(|n| n.is_finite() && n >= 0.)
                && self.animator.time.is_finite()
                && self.animator.time >= 0.
                && (0.0..=1.).contains(&self.animator.transition),
            "Invalid saved Alice animation"
        );
        anyhow::ensure!(
            self.face_time.is_finite() && self.face_time >= 0.,
            "Invalid saved facial clock"
        );
        self.viewmodel.validate()
    }
    /// What `Character::restore` followed by `Restored::build`'s `power_appearance` leaves in
    /// Alice, computed without a window: the same validation, the real `Actions::restore` on
    /// `actions` (a rig of `bones` bones), and the same upgrades. The idle-gesture clip lookup
    /// of `Acting::restore` needs the loaded animations and is not repeated.
    pub fn restored_headless(
        &self,
        bones: usize,
        actions: &mut crate::weapons::Actions,
        stats: &crate::inventory::Stats,
    ) -> Result<Self> {
        self.validate(bones)?;
        self.projectiles.validate()?;
        actions.restore(&self.actions)?;
        let mut out = self.clone();
        out.projectiles = self.projectiles.migrated();
        out.power.sync(power_form(stats));
        if !stats.alive() {
            out.projectiles = out.projectiles.dismissed();
        }
        if stats.equipped().is_none() {
            actions.unarm(&self.animator.pose);
        }
        out.actions = actions.snapshot();
        Ok(out)
    }
    /// The block a restored Alice saves again for a player with these stats.
    ///
    /// Restoring never rewrites a valid block, except for these deliberate upgrades of state
    /// that older saves lack: the power-up presentation absent from saves before the
    /// transformation performances is initialised to the stats' current form without
    /// replaying its pickup performance (`power_appearance`); an unequipped player drops the
    /// toy pose; a Blade cooldown from before the recovery clock becomes that clock; legacy
    /// projectiles take their current kinds; and a dead player's summon is dismissed. Every
    /// other field must come back exactly as saved, so legacy-fixture checks compare against
    /// this and never against the raw old block.
    pub fn migrated(&self, stats: &crate::inventory::Stats) -> Self {
        let mut s = self.clone();
        s.power.sync(power_form(stats));
        s.actions = s.actions.migrated();
        if stats.equipped().is_none() {
            s.actions = s.actions.unarmed(&s.animator.pose);
        }
        s.projectiles = s.projectiles.migrated();
        if !stats.alive() {
            s.projectiles = s.projectiles.dismissed();
        }
        s
    }
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct AnimatorSave {
    #[serde(default)]
    last_feet: Option<Vec3>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_rope_length: Option<f32>,
    motion: Motion,
    time: f32,
    transition: f32,
    previous: Vec<Transform>,
    pose: Vec<Transform>,
    jumps: u64,
    landings: u64,
    large: bool,
    offset: f32,
    previous_offset: f32,
    #[serde(default = "default_death_clip")]
    death_clip: usize,
}
fn default_death_clip() -> usize {
    DEATH_FAINT
}
/// One GPU pipeline, with effects explicitly owned by each draw. Do not expose
/// the raw Material: a queued flush can restore an earlier actor's uniforms.
#[derive(Clone)]
pub(crate) struct SkinMaterial(std::rc::Rc<Material>);

#[derive(Clone, Copy, Default, PartialEq)]
struct Appearance {
    dissolve: f32,
    ghost: bool,
    power: f32,
}

impl SkinMaterial {
    pub(crate) fn atmosphere(&self, atmosphere: &crate::environment::Atmosphere, eye: Vec3) {
        atmosphere.apply(&self.0, eye);
    }

    /// Every ordinary actor/prop starts opaque, even after a frame/readback flush.
    pub(crate) fn bind(&self) {
        self.bind_appearance(Appearance::default());
    }

    fn bind_appearance(&self, appearance: Appearance) {
        crate::render_fx::appearance(crate::render_fx::Appearance {
            dissolve: appearance.dissolve,
            ghost: appearance.ghost,
            power: appearance.power,
        });
        self.0.set_uniform("Dissolve", appearance.dissolve);
        self.0
            .set_uniform("Ghost", if appearance.ghost { 1_f32 } else { 0_f32 });
        self.0.set_uniform("Power", appearance.power);
        gl_use_material(&self.0);
    }

    fn draw(&self, appearance: Appearance, draw: impl FnOnce()) {
        self.bind_appearance(appearance);
        draw();
        if appearance != Appearance::default() {
            // Macroquad writes queued uniforms back into the shared pipeline.
            // Flush before resetting, including when no later actor is drawn.
            unsafe {
                get_internal_gl().flush();
            }
            self.bind();
        }
    }

    pub(crate) fn draw_dissolving(&self, amount: f32, draw: impl FnOnce()) {
        self.draw(
            Appearance {
                dissolve: amount.clamp(0., 1.),
                ..Default::default()
            },
            draw,
        );
    }
}

/// Staged screenshots exercise the same skin, attachment and gesture rendering as play.
pub async fn render_animations(assets: &mut Assets) -> Result<()> {
    let mut alice = Character::load(assets)?;
    let mut legacy = serde_json::to_value(alice.snapshot())?;
    legacy.as_object_mut().unwrap().remove("acting");
    alice.restore(&serde_json::from_value(legacy)?)?;
    let specs = texture::read_materials(assets)?;
    for (clip, time, weapon) in [
        ("knife_idle3", 1., 0),
        ("cards_idle2", 2., 1),
        ("mallet_idle3", 1.5, 2),
        ("jacks_idle2", 0.8, 5),
        ("dice_idle2", 2., 6),
        ("staff_idle2", 0.8, 7),
        ("talk_gnomes_01", 0.8, crate::weapons::UNARMED),
        ("pain_front", 0.2, 0),
    ] {
        alice.actions.selected = weapon;
        alice.actions.playing = None;
        alice.acting.stage(clip, time, weapon)?;
        alice.actions.pose = alice.animator.pose.clone();
        alice
            .acting
            .apply(&mut alice.actions.pose, &alice.upper, true);
        let pose = alice.skeleton.global_pose(&alice.actions.pose);
        alice.anchors = alice.tags.map(|i| Transform {
            rotation: pose[i].rotation,
            translation: pose[i].translation * alice.scale,
        });
        for frame in 0..3 {
            animation_camera(clip);
            alice.draw(Vec3::ZERO, true);
            if frame == 2 {
                crate::viewer::save_capture(std::path::Path::new(&format!(
                    "private/animation-{clip}.png"
                )))?;
            }
            next_frame().await;
        }
    }
    for name in ["c_torchgnome", "c_cheshire", "c_madhatter"] {
        let mut actor = crate::npc::Puppet::load(assets, name, &[], &specs)?;
        let def = Definition::load(assets, &format!("models/{name}.tik"))?;
        let skeleton = Skeleton::parse(&assets.read(&format!("{}/{}", def.path, def.model))?)?;
        let idle = Animation::parse(
            &assets.read(&format!(
                "{}/{}",
                def.path,
                def.animations[actor.idle_clip()]
            ))?,
            skeleton.bones.len(),
        )?;
        let (min, max) = (
            idle.frames[0].min * def.scale,
            idle.frames[0].max * def.scale,
        );
        for (label, speaking, time) in [("rest", false, 0.), ("gesture", true, 3.)] {
            for frame in 0..3 {
                animation_camera_bounds(name, min, max);
                alice.material.bind();
                actor.draw_performance(
                    speaking,
                    time,
                    Transform {
                        translation: Vec3::ZERO,
                        rotation: Quat::IDENTITY,
                    },
                    1.,
                    true,
                );
                gl_use_default_material();
                if frame == 2 {
                    crate::viewer::save_capture(std::path::Path::new(&format!(
                        "private/animation-{name}-{label}.png"
                    )))?;
                }
                next_frame().await;
            }
        }
    }
    let school = crate::bsp::Bsp::parse(&assets.read("maps/skool1.bsp")?)?;
    let books = school
        .entities
        .iter()
        .find(|e| e.get("targetname").is_some_and(|n| n == "book_stack"))
        .and_then(|e| e.get("model"))
        .context("Missing authored book stack")?
        .trim_start_matches("models/")
        .trim_end_matches(".tik");
    for (name, clip) in [(books, "sway"), ("sky_watch", "handsmove")] {
        let def = Definition::load(assets, &format!("models/{name}.tik"))?;
        let model = crate::ambient_animation::read(assets, &def, clip)?;
        let (min, max) = model
            .surfaces
            .iter()
            .flat_map(|s| s.frames.iter().flatten())
            .fold(
                (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
                |(min, max), p| (min.min(*p * def.scale), max.max(*p * def.scale)),
            );
        let mut prop = crate::weapons::Prop::load_animation(assets, name, clip, &specs)?;
        for (label, time) in [("start", 0.), ("moving", 1.5)] {
            for frame in 0..3 {
                animation_camera_bounds(name, min, max);
                alice.material.bind();
                prop.draw_frame(
                    Transform {
                        translation: Vec3::ZERO,
                        rotation: Quat::IDENTITY,
                    },
                    1.,
                    true,
                    time,
                    true,
                );
                gl_use_default_material();
                if frame == 2 {
                    crate::viewer::save_capture(std::path::Path::new(&format!(
                        "private/animation-{name}-{label}.png"
                    )))?;
                }
                next_frame().await;
            }
        }
    }
    println!("PASS animation render: eight Alice gestures with props, three NPCs and two moving scenery models");
    Ok(())
}
fn animation_camera(label: &str) {
    animation_camera_bounds(label, vec3(-25., -25., 0.), vec3(25., 25., 80.));
}
fn animation_camera_bounds(label: &str, min: Vec3, max: Vec3) {
    clear_background(Color::new(0.10, 0.12, 0.15, 1.));
    set_default_camera();
    draw_text(label, 20., 30., 24., WHITE);
    let target = (min + max) * 0.5;
    let distance = (max - min).length().max(1.) * 1.8;
    set_camera(&Camera3D {
        position: target + vec3(1., -1., 0.35).normalize() * distance,
        target,
        up: Vec3::Z,
        fovy: 40_f32.to_radians(),
        z_near: 0.5,
        z_far: distance * 10.,
        ..Default::default()
    });
}
thread_local! {
    // Every actor/prop uses this same pipeline. Keep ownership weak in the cache
    // so it is released with the last scene, but never allocate one per actor or
    // duplicate it while a quickload retains the previous complete game.
    static SKIN_MATERIAL: std::cell::RefCell<std::rc::Weak<Material>> = Default::default();
}
pub(crate) fn skin_material() -> Result<SkinMaterial> {
    SKIN_MATERIAL.with(|cache| {
        if let Some(material) = cache.borrow().upgrade() {
            return Ok(SkinMaterial(material));
        }
        let material = load_material(
            ShaderSource::Glsl {
                vertex: VERTEX,
                fragment: &crate::environment::fragment(FRAGMENT),
            },
            MaterialParams {
                uniforms: {
                    let mut u = crate::environment::uniforms();
                    u.push(UniformDesc::new("Dissolve", UniformType::Float1));
                    u.push(UniformDesc::new("Ghost", UniformType::Float1));
                    u.push(UniformDesc::new("Power", UniformType::Float1));
                    u
                },
                pipeline_params: PipelineParams {
                    depth_test: Comparison::LessOrEqual,
                    depth_write: true,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .map_err(|e| anyhow::anyhow!("Character shader: {e:?}"))?;
        let material = std::rc::Rc::new(material);
        *cache.borrow_mut() = std::rc::Rc::downgrade(&material);
        Ok(SkinMaterial(material))
    })
}

/// Shared pipelines must retain per-draw uniforms: Alice's powers must not tint
/// later NPCs/props, and no owner may consume another pipeline slot.
pub async fn check_shared_material() -> Result<()> {
    let owners = (0..64)
        .map(|_| skin_material())
        .collect::<Result<Vec<_>>>()?;
    anyhow::ensure!(
        owners
            .iter()
            .all(|m| std::rc::Rc::ptr_eq(&m.0, &owners[0].0)),
        "Actor shaders were duplicated"
    );
    clear_background(BLACK);
    let camera = Camera3D {
        position: vec3(0., 0., 10.),
        target: Vec3::ZERO,
        up: Vec3::Y,
        projection: Projection::Orthographics,
        fovy: 4.,
        z_near: 0.1,
        z_far: 50.,
        ..Default::default()
    };
    set_camera(&camera);
    let white = Texture2D::from_rgba8(1, 1, &[255; 4]);
    let quad = |x: f32| Mesh {
        vertices: [
            vec3(x - 0.7, -1., 0.),
            vec3(x + 0.7, -1., 0.),
            vec3(x + 0.7, 1., 0.),
            vec3(x - 0.7, 1., 0.),
        ]
        .into_iter()
        .map(|position| Vertex {
            position,
            uv: Vec2::ZERO,
            normal: Vec4::ZERO,
            color: [255; 4],
        })
        .collect(),
        indices: vec![0, 1, 2, 0, 2, 3],
        texture: Some(white.clone()),
    };
    for (index, (x, power)) in [(-1_f32, 1_f32), (1., 0.)].into_iter().enumerate() {
        let m = &owners[index];
        m.atmosphere(
            &crate::environment::Atmosphere::default(),
            vec3(0., 0., 10.),
        );
        m.bind_appearance(Appearance {
            power,
            ..Default::default()
        });
        draw_mesh(&quad(x));
    }
    let frame = get_screen_data();
    let sample = |x: f32| {
        frame.get_pixel(
            (frame.width as f32 * 0.5 + x * frame.height as f32 / 4.) as u32,
            frame.height as u32 / 2,
        )
    };
    let (powered, plain) = (sample(-1.), sample(1.));
    anyhow::ensure!(
        powered.r > 0.98
            && powered.g < 0.8
            && powered.b < 0.7
            && plain.r > 0.98
            && plain.g > 0.98
            && plain.b > 0.98,
        "Shared character power leaked between draws: {powered:?} / {plain:?}"
    );
    gl_use_default_material();
    set_default_camera();
    next_frame().await;
    for amount in [0.5_f32, 1.] {
        clear_background(BLACK);
        set_camera(&camera);
        let material = &owners[0];
        material.draw_dissolving(amount, || draw_mesh(&quad(0.)));
        let faded = get_screen_data();
        let mut visible = 0;
        for y in 0..32 {
            for x in 0..32 {
                visible += usize::from(
                    faded
                        .get_pixel(faded.width as u32 / 2 + x, faded.height as u32 / 2 + y)
                        .r
                        > 0.9,
                );
            }
        }
        anyhow::ensure!(
            if amount == 1. {
                visible == 0
            } else {
                visible == 512
            },
            "Cat dissolve lost its intended coverage: {amount}, {visible} pixels"
        );
        next_frame().await;
        clear_background(BLACK);
        set_camera(&camera);
        // Another actor shares this material and must not inherit the prior frame's fade.
        owners[1].bind();
        draw_mesh(&quad(0.));
        let restored = get_screen_data();
        anyhow::ensure!(
            restored
                .get_pixel(restored.width as u32 / 2, restored.height as u32 / 2)
                .r
                > 0.98,
            "Cat fade leaked into another actor after the frame boundary"
        );
        next_frame().await;
    }
    gl_use_default_material();
    set_default_camera();
    println!("PASS 64 character/prop owners share one pipeline; power tint remains per draw");
    println!("PASS partial/full Cat dissolve clears before the next actor/frame");
    Ok(())
}

/// GPU regression using the actual player and cinematic Alice meshes. Deliberately
/// leave stale uniforms behind, as a final effect draw or readback can do.
pub async fn check_visibility(assets: &mut Assets) -> Result<()> {
    check_shared_material().await?;
    for queued in [false, true] {
        let material = skin_material()?;
        let mut alice = Character::load(assets)?;
        let specs = texture::read_materials(assets)?;
        let mut puppet = crate::npc::Puppet::load(assets, "alice", &["idle_stand"], &specs)?;
        let player = crate::movement::Player::new(Vec3::ZERO);
        alice.reset(&player, 0.);
        let mut counts = Vec::new();
        let mut rendered = Vec::new();
        for poison in [
            Appearance::default(),
            Appearance {
                dissolve: 1.,
                ghost: true,
                power: 2.,
            },
        ] {
            for kind in 0..4 {
                material.bind_appearance(poison);
                // Survives a frame boundary and a readback before the actor binds.
                next_frame().await;
                visibility_camera(Vec3::ZERO, 1.);
                if !queued {
                    crate::render_fx::finish();
                }
                let before = get_screen_data();
                material.bind_appearance(poison);
                material.atmosphere(&crate::environment::Atmosphere::default(), Vec3::ZERO);
                match kind {
                    0 => alice.draw(Vec3::ZERO, true),
                    1 => alice.draw_ghost(Vec3::ZERO, true, false),
                    2 => puppet.draw(
                        "idle_stand",
                        0.,
                        true,
                        Transform {
                            translation: Vec3::ZERO,
                            rotation: Quat::IDENTITY,
                        },
                        1.,
                        true,
                    ),
                    _ => puppet.draw_performance(
                        false,
                        0.,
                        Transform {
                            translation: Vec3::ZERO,
                            rotation: Quat::IDENTITY,
                        },
                        1.,
                        true,
                    ),
                }
                let after = visibility_image();
                let pixels = visible_pixels(&before, &after);
                anyhow::ensure!(
                    pixels > 200,
                    "Alice draw path {kind} invisible: {pixels} pixels"
                );
                counts.push(pixels);
                rendered.push(after.bytes);
                next_frame().await;
            }
        }
        anyhow::ensure!(
            counts[..4] == counts[4..] && rendered[..4] == rendered[4..],
            "Effects changed later Alice coverage: {counts:?}"
        );
        // Intentional invisibility remains partial; it must not affect the next body.
        visibility_camera(Vec3::ZERO, 1.);
        if !queued {
            crate::render_fx::finish();
        }
        let before = get_screen_data();
        alice.draw_ghost(Vec3::ZERO, true, true);
        let ghost = visible_pixels(&before, &visibility_image());
        anyhow::ensure!(
            ghost > 0 && ghost < counts[0] / 2,
            "Glass ghost coverage invalid: {ghost}"
        );
        next_frame().await;
        visibility_camera(Vec3::ZERO, 1.);
        if !queued {
            crate::render_fx::finish();
        }
        let before = get_screen_data();
        alice.draw(Vec3::ZERO, true);
        anyhow::ensure!(
            visible_pixels(&before, &visibility_image()) == counts[0],
            "Ghost leaked to next frame"
        );
        gl_use_default_material();
        set_default_camera();
        next_frame().await;
        println!("PASS actual Alice/player/puppet meshes recover from stale dissolve, ghost and power at frame/readback boundaries; intentional Glass ghost preserved (queued={queued})");
        let end_pose = puppet
            .handoff_pose()
            .context("Missing cinematic handoff pose")?;
        alice.resume_scene(&player, 0., Some(end_pose));
        anyhow::ensure!(
            alice
                .animator
                .pose
                .iter()
                .zip(&end_pose.local)
                .all(|(a, b)| a.translation == b.translation && a.rotation == b.rotation),
            "Cinematic pose was lost before gameplay resumed"
        );
        let saved = alice.snapshot();
        alice.restore(&serde_json::from_slice(&serde_json::to_vec(&saved)?)?)?;
        anyhow::ensure!(
            alice.animator.transition == 0.,
            "Pose blend lost after saving"
        );
        visibility_camera(player.feet, 1.);
        let before = get_screen_data();
        alice.draw(player.feet, true);
        anyhow::ensure!(
            visible_pixels(&before, &visibility_image()) > 200,
            "Alice vanished during pose handoff"
        );
        next_frame().await;
        println!("PASS cinematic pose survives handoff and saved blend (queued={queued})");
    }
    Ok(())
}

pub(crate) fn visibility_camera(feet: Vec3, scale: f32) {
    clear_background(BLACK);
    let scale = scale.max(0.1);
    let target = feet + Vec3::Z * 35. * scale;
    set_camera(&Camera3D {
        position: target + vec3(120., -140., 45.) * scale,
        target,
        up: Vec3::Z,
        fovy: 50_f32.to_radians(),
        z_near: 0.1,
        z_far: 30000.,
        ..Default::default()
    });
    let eye = target + vec3(120., -140., 45.) * scale;
    crate::render_fx::begin(
        eye,
        (target - eye).normalize_or_zero(),
        0.,
        &crate::environment::Atmosphere::default(),
        true,
    );
}

pub(crate) fn visibility_image() -> Image {
    if crate::render_fx::active() {
        crate::render_fx::finish();
    }
    get_screen_data()
}

pub(crate) fn visible_pixels(before: &Image, after: &Image) -> usize {
    before
        .bytes
        .chunks_exact(4)
        .zip(after.bytes.chunks_exact(4))
        .filter(|(a, b)| a[..3] != b[..3])
        .count()
}

pub(crate) fn draw_skin(
    skeleton: &Skeleton,
    meshes: &mut [Option<Mesh>],
    pose: &[Transform],
    transform: Transform,
    scale: f32,
    fullbright: bool,
) {
    draw_skin_scaled(skeleton, meshes, pose, transform, scale, fullbright, &[]);
}
pub(crate) fn draw_skin_scaled(
    skeleton: &Skeleton, meshes: &mut [Option<Mesh>], pose: &[Transform],
    transform: Transform, scale: f32, fullbright: bool, bone_scales: &[f32],
) {
    draw_skin_deformed(skeleton, meshes, pose, transform, scale, fullbright, (bone_scales, 1.));
}
pub(crate) fn draw_skin_alpha(
    skeleton: &Skeleton, meshes: &mut [Option<Mesh>], pose: &[Transform],
    transform: Transform, scale: f32, fullbright: bool, alpha: f32,
) {
    draw_skin_deformed(skeleton, meshes, pose, transform, scale, fullbright, (&[], alpha));
}
fn draw_skin_deformed(
    skeleton: &Skeleton, meshes: &mut [Option<Mesh>], pose: &[Transform],
    transform: Transform, scale: f32, fullbright: bool, (bone_scales, alpha): (&[f32], f32),
) {
    for (surface, mesh) in skeleton.surfaces.iter().zip(meshes) {
        let Some(mesh) = mesh else {
            continue;
        };
        for (src, dst) in surface.vertices.iter().zip(&mut mesh.vertices) {
            let position = if bone_scales.is_empty() { src.position(pose) } else {
                src.weights.iter().map(|w| pose[w.bone].point(w.offset * bone_scales[w.bone]) * w.amount).sum()
            };
            dst.position = transform.point(position * scale);
            dst.normal = Vec4::ZERO;
        }
        for tri in mesh.indices.chunks_exact(3) {
            let [a, b, c] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
            let n = (mesh.vertices[b].position - mesh.vertices[a].position)
                .cross(mesh.vertices[c].position - mesh.vertices[a].position);
            for i in [a, b, c] {
                mesh.vertices[i].normal += n.extend(0.);
            }
        }
        for v in &mut mesh.vertices {
            let n = v.normal.truncate().normalize_or_zero();
            let light = if fullbright {
                1.
            } else {
                0.65 + 0.35 * n.dot(vec3(0.3, -0.5, 1.).normalize()).max(0.)
            };
            v.color = [
                (light * 255.) as u8,
                (light * 255.) as u8,
                (light * 255.) as u8,
                (alpha.clamp(0., 1.) * 255.) as u8,
            ];
        }
        crate::render_fx::skin(mesh);
    }
}

/// Sweep a small camera box along the view arm to avoid looking through walls.
pub fn follow_camera(world: &World, feet: Vec3, direction: Vec3) -> (Vec3, Vec3, bool) {
    follow_camera_at(world, feet, direction, 132.)
}
pub fn follow_camera_at(
    world: &World,
    feet: Vec3,
    direction: Vec3,
    distance: f32,
) -> (Vec3, Vec3, bool) {
    crate::camera::Follow::default().update(world, feet, direction, distance, 0.)
}

pub(crate) const VERTEX: &str = r#"#version 100
attribute vec3 position;
attribute vec2 texcoord;
attribute vec4 color0;
uniform mat4 Model;
uniform mat4 Projection;
varying highp vec2 uv;
varying lowp vec4 color;
varying highp vec3 worldPosition;
void main() { worldPosition=(Model*vec4(position,1.0)).xyz; gl_Position = Projection * Model * vec4(position,1.0); uv=texcoord; color=color0/255.0; }
"#;
const FRAGMENT: &str = r#"#version 100
precision mediump float;
uniform sampler2D Texture;
varying highp vec2 uv;
varying lowp vec4 color;
// FOG
uniform float Dissolve;
uniform float Ghost;
uniform float Power;
void main() { if(Dissolve>0.0 && (mod(floor(gl_FragCoord.x)*3.0+floor(gl_FragCoord.y)*5.0,16.0)+0.5)/16.0<Dissolve) discard; vec4 t=texture2D(Texture,uv); if(t.a<0.35) discard; if(Ghost>0.5 && mod(floor(gl_FragCoord.x)+floor(gl_FragCoord.y),4.0)>0.5) discard; vec3 c=t.rgb*color.rgb; if(Power>1.5) c=mix(c,c*vec3(0.65,1.15,0.7),0.6); else if(Power>0.5) c=mix(c,c*vec3(1.3,0.55,0.4),0.6); gl_FragColor=vec4(fogged(c,0.0),1.0); }
"#;

/// Real clips, real swept controller, with presentation forbidden to move the body.
pub fn check_movement(assets: &mut Assets) -> Result<()> {
    use crate::movement::{Controls, FIXED_DT};
    use anyhow::ensure;
    let def = Definition::alice(assets)?;
    let skeleton = Skeleton::parse(&assets.read(&format!("{}/{}", def.path, def.model))?)?;
    let mut clips = Vec::new();
    for name in CLIPS {
        clips.push(Animation::parse(
            &assets.read(&format!("{}/{name}.ska", def.path))?,
            skeleton.bones.len(),
        )?);
    }
    for i in [1, 2, 7, 8, 9, 10, 22, 23, 30, 31] {
        let clip = &clips[i];
        let vertical = i >= 22;
        let distance = if vertical {
            clip.frames.iter().map(|f| f.delta.z.abs()).sum()
        } else {
            clip.distance
        };
        ensure!(distance > 0., "Missing movement data: {}", CLIPS[i]);
        let once = clip.advance_distance(0., distance * 2.37, vertical);
        for hz in [30, 60, 144] {
            let mut time = 0.;
            for _ in 0..hz {
                time = clip.advance_distance(time, distance * 2.37 / hz as f32, vertical);
            }
            ensure!(
                (once - time).abs() < 0.002,
                "Stride depends on frame rate: {} at {hz} Hz: {once} vs {time}",
                CLIPS[i]
            );
        }
    }
    crate::footing::check(&skeleton, &clips[0], &clips[7], def.scale)?;
    let mut a = Animator::new(clips);
    a.scale = def.scale;
    // Original paired strokes must keep the same phase with and without swing,
    // including render frames in which the fixed controller does not advance.
    for rise in [-1., 1.] {
        let mut reference: Option<f32> = None;
        for hz in [30, 60, 144] {
            let mut world = World::fixture(&[]);
            let anchor = Vec3::Z * 1000.;
            world.traversal.ropes.push(crate::traversal::Rope {
                id: crate::entity::Id(1),
                model: 1,
                enabled: true,
                origin: Vec3::ZERO,
                anchor,
                length: 1000.,
            });
            let mut p =
                Player::new(anchor + vec3(0.15, 0., -1.).normalize() * 500. - Vec3::Z * 40.);
            p.script_motion = 1;
            p.rope = Some(crate::movement::RopeGrip {
                id: crate::entity::Id(1),
                length: 500.,
                rise,
            });
            let mut clock = crate::movement::FixedClock::default();
            a.reset(&p);
            let index = if rise > 0. { 22 } else { 23 };
            let travel: f32 = [index, rope_pair(index).unwrap()]
                .into_iter()
                .flat_map(|i| &a.clips[i].frames)
                .map(|f| f.delta.z.abs())
                .sum();
            let mut phase = 0.;
            for _ in 0..hz * 2 {
                clock.advance(
                    1. / hz as f64,
                    &world,
                    &mut p,
                    Controls {
                        rise,
                        wish: Vec2::Y,
                        ..Default::default()
                    },
                );
                a.update(1. / hz as f32, &p, false);
                ensure!(
                    a.sound_clip == Some(index),
                    "Climb flickered to hang at {hz} Hz"
                );
                let span = a.sound_span.unwrap().1;
                phase += span.end - span.start;
            }
            let distance = (p.rope.as_ref().unwrap().length - 500.).abs();
            ensure!(
                (phase - distance / a.scale / travel * a.duration(index)).abs() < 0.002,
                "Swing changed climb cadence at {hz} Hz"
            );
            if let Some(previous) = reference {
                ensure!(
                    (phase - previous).abs() < 0.002,
                    "Climb cadence depends on render rate"
                );
            }
            reference = Some(phase);
        }
    }
    for (unarmed, large, index) in [(true, false, 1), (false, false, 7), (false, true, 9)] {
        a.unarmed = unarmed;
        a.large = large;
        println!(
            "Walk {}: {:.3} units in {:.3} s",
            CLIPS[index],
            a.clips[index].distance,
            a.duration(index)
        );
        for fraction in [0.125, 0.25, 0.5, 1., 2.] {
            let mut reference: Option<f32> = None;
            for hz in [30, 60, 144] {
                let world =
                    World::fixture(&[(vec3(-2000., -2000., -100.), vec3(2000., 2000., 0.))]);
                let mut p = Player::new(vec3(0., 0., 0.1));
                p.grounded = true;
                p.tea = fraction > 1.;
                p.velocity.x = crate::movement::WALK_SPEED * fraction;
                let mut clock = crate::movement::FixedClock::default();
                a.reset(&p);
                let mut elapsed = 0.;
                // Compare brisk cadence with real travel, including slow analog
                // input and render frames without a physics tick.
                for _ in 0..hz * 8 {
                    clock.advance(
                        1. / hz as f64,
                        &world,
                        &mut p,
                        Controls {
                            wish: Vec2::X * fraction.min(1.),
                            ..Default::default()
                        },
                    );
                    a.update(1. / hz as f32, &p, false);
                    if clock.ticks > 0 {
                        ensure!(
                            a.motion == Motion::Walk && a.sound_clip == Some(index),
                            "Gait stopped between physics ticks at {hz} Hz"
                        );
                        let span = a.sound_span.unwrap().1;
                        elapsed += span.end - span.start;
                    }
                }
                ensure!(
                    (p.feet.x - 8. * crate::movement::WALK_SPEED * fraction).abs() < 0.1,
                    "Walking speed changed at {hz} Hz"
                );
                let clip = &a.clips[index];
                let expected = clip.advance_distance(0., p.feet.x / a.scale, false);
                ensure!(
                    (elapsed - expected).abs() < 0.002,
                    "{} stride slipped at {fraction} input, {hz} Hz: {elapsed} vs {expected}",
                    CLIPS[index]
                );
                // Per-frame root curves are uneven (including stationary frames).
                // Check average playback over a complete stride, not the phase of
                // the short partial stride left at the end of this controller run.
                let rate = p.velocity.x / a.scale / clip.distance * clip.duration();
                ensure!(
                    (rate / (2. * fraction) - 1.).abs() < 0.02,
                    "{} full-stride cadence is {rate} at {fraction} input",
                    CLIPS[index]
                );
                if let Some(previous) = reference {
                    ensure!(
                        (elapsed - previous).abs() < 0.002,
                        "Walk cadence depends on render rate"
                    );
                }
                reference = Some(elapsed);
            }
        }
    }
    a.unarmed = false;
    a.large = false;
    let floor = (vec3(-2000., -2000., -100.), vec3(2000., 2000., 0.));
    let wall = World::fixture(&[floor, (vec3(180., -1000., 0.), vec3(200., 1000., 300.))]);
    let mut p = Player::new(vec3(0., 0., 0.1));
    a.reset(&p);
    let mut seen = std::collections::BTreeSet::new();
    for frame in 0..480 {
        p.tick(
            &wall,
            Controls {
                wish: Vec2::X,
                run: frame >= 55,
                jump: frame == 75,
                ..Default::default()
            },
        );
        let before = serde_json::to_vec(&p)?;
        a.update(FIXED_DT, &p, frame >= 55);
        ensure!(
            before == serde_json::to_vec(&p)? && wall.body_clear(p.feet),
            "Animation changed collision or entered solid"
        );
        seen.insert(format!("{:?}", a.motion));
        if frame == 130 {
            let saved = serde_json::to_vec(&a.snapshot())?;
            a.update(0., &p, true);
            ensure!(
                saved == serde_json::to_vec(&a.snapshot())?,
                "Paused locomotion changed"
            );
            a.restore(&serde_json::from_slice(&saved)?);
        }
    }
    for name in ["Walk", "Run", "Takeoff", "Air", "Idle"] {
        ensure!(
            seen.contains(name),
            "Missing movement phase {name}: {seen:?}"
        );
    }
    ensure!(
        p.feet.x < 165. && a.motion == Motion::Idle,
        "Blocked player kept walking"
    );
    let ledge = World::fixture(&[floor, (vec3(40., -100., 0.), vec3(180., 100., 48.))]);
    p = Player::new(vec3(0., 0., 0.1));
    a.reset(&p);
    let mut climbed = false;
    for _ in 0..240 {
        p.tick(
            &ledge,
            Controls {
                wish: Vec2::X,
                jump: true,
                ..Default::default()
            },
        );
        a.update(FIXED_DT, &p, false);
        ensure!(ledge.body_clear(p.feet), "Climb entered collision");
        if let Some(progress) = p.climb_progress() {
            climbed = true;
            ensure!(a.motion == Motion::Climb, "Climb clip missing");
            let duration = a.clips[a.sound_clip.unwrap()].duration();
            ensure!(
                (a.time - progress * duration).abs() < 0.001,
                "Climb animation fell behind controller"
            );
        }
    }
    ensure!(climbed, "Climb fixture never caught ledge");
    // Pandemonium owns its grip separately from Player::rope. Swinging changes
    // body height too, so only resolved travel along the rope selects a climb.
    p.rope = None;
    p.script_motion = 1;
    p.script_rope_length = Some(200.);
    for (rise, dz, expected) in [(1., 0.5, 22), (-1., -0.5, 23), (0., 0.5, 19), (0., 0., 19)] {
        p.script_rope_rise = rise;
        a.reset(&p);
        for _ in 0..60 {
            p.feet.z += dz;
            p.script_rope_length = p.script_rope_length.map(|l| l - rise * 0.5);
            a.update(FIXED_DT, &p, false);
        }
        ensure!(
            a.sound_clip == Some(expected),
            "Script rope selected wrong clip"
        );
    }
    for mode in [1, 2, 3] {
        p.script_motion = mode;
        p.rope = Some(crate::movement::RopeGrip {
            id: crate::entity::Id(0),
            length: 100.,
            rise: 1.,
        });
        a.reset(&p);
        for _ in 0..60 {
            p.feet += Vec3::Z * 0.5;
            a.update(FIXED_DT, &p, false);
        }
        ensure!(
            matches!(a.motion, Motion::Rope | Motion::Cart | Motion::Airship),
            "Missing script pose"
        );
        let saved = a.snapshot();
        let original = serde_json::to_vec(&saved)?;
        a.update(0., &p, false);
        ensure!(
            original == serde_json::to_vec(&a.snapshot())?,
            "Paused script pose changed"
        );
        p.feet += Vec3::Z * 0.5;
        a.update(FIXED_DT, &p, false);
        let expected = serde_json::to_vec(&a.snapshot())?;
        a.restore(&serde_json::from_slice(&original)?);
        a.update(FIXED_DT, &p, false);
        ensure!(
            expected == serde_json::to_vec(&a.snapshot())?,
            "Restored script movement differs"
        );
    }
    println!("PASS original stride curves and paired rope strokes at 30/60/144 Hz; swept walk/run/jump/wall/climb; rope/cart/airship pause and save continuation");
    Ok(())
}

pub async fn render_movement(assets: &mut Assets) -> Result<()> {
    use crate::movement::{Controls, FIXED_DT};
    use anyhow::ensure;
    check_movement(assets)?;
    let mut alice = Character::load(assets)?;
    let floor = (vec3(-2000., -2000., -100.), vec3(2000., 2000., 0.));
    for (label, solid, steps, jump) in [
        (
            "walk",
            (vec3(400., -100., 0.), vec3(450., 100., 300.)),
            43,
            false,
        ),
        (
            "wall",
            (vec3(65., -100., 0.), vec3(90., 100., 300.)),
            130,
            false,
        ),
        (
            "jump",
            (vec3(400., -100., 0.), vec3(450., 100., 300.)),
            25,
            true,
        ),
        (
            "climb",
            (vec3(40., -100., 0.), vec3(180., 100., 48.)),
            32,
            true,
        ),
    ] {
        let world = World::fixture(&[floor, solid]);
        let context = crate::combat::Context {
            world: &world,
            targets: &[],
        };
        let mut p = Player::new(vec3(0., 0., 0.1));
        alice.reset(&p, 0.);
        for frame in 0..steps {
            p.tick(
                &world,
                Controls {
                    wish: Vec2::X,
                    jump: jump && (frame == 1 || label == "climb"),
                    ..Default::default()
                },
            );
            alice.update(
                FIXED_DT,
                &p,
                false,
                WeaponInput {
                    selected: crate::weapons::UNARMED,
                    dice: 0,
                    click: None,
                    aim: Vec3::X,
                    first_person: false,
                },
                &context,
            );
        }
        let saved = serde_json::to_vec(&alice.snapshot())?;
        alice.update(
            0.,
            &p,
            false,
            WeaponInput {
                selected: crate::weapons::UNARMED,
                dice: 0,
                click: None,
                aim: Vec3::X,
                first_person: false,
            },
            &context,
        );
        ensure!(
            saved == serde_json::to_vec(&alice.snapshot())?,
            "Paused feet/pose changed"
        );
        let draw = |alice: &mut Character| {
            animation_camera_bounds(
                label,
                p.feet + vec3(-70., -70., 0.),
                p.feet + vec3(70., 70., 110.),
            );
            draw_cube(
                vec3(p.feet.x, p.feet.y, -1.),
                vec3(500., 500., 2.),
                None,
                Color::new(0.18, 0.22, 0.23, 1.),
            );
            draw_cube(
                (solid.0 + solid.1) * 0.5,
                solid.1 - solid.0,
                None,
                Color::new(0.28, 0.3, 0.32, 1.),
            );
            alice.draw(p.feet, true);
            gl_use_default_material();
        };
        for frame in 0..3 {
            draw(&mut alice);
            if frame == 2 {
                crate::viewer::save_capture(std::path::Path::new(&format!(
                    "private/movement-{label}.png"
                )))?;
            }
            next_frame().await;
        }
        draw(&mut alice);
        let original = get_screen_data();
        next_frame().await;
        alice.restore(&serde_json::from_slice(&saved)?)?;
        draw(&mut alice);
        let restored = get_screen_data();
        next_frame().await;
        ensure!(
            original.bytes == restored.bytes,
            "Restored {label} pose rendered differently"
        );
    }
    let world = World::fixture(&[]);
    let context = crate::combat::Context {
        world: &world,
        targets: &[],
    };
    for (name, rise) in [("up", 1.), ("down", -1.)] {
        let mut p = Player::new(Vec3::ZERO);
        p.script_motion = 1;
        p.script_rope_length = Some(500.);
        p.script_rope_rise = rise;
        alice.reset(&p, 0.);
        for frame in 0..120 {
            p.script_rope_length = p.script_rope_length.map(|l| l - rise * 100. * FIXED_DT);
            alice.update(
                FIXED_DT,
                &p,
                false,
                WeaponInput {
                    selected: crate::weapons::UNARMED,
                    dice: 0,
                    click: None,
                    aim: Vec3::X,
                    first_person: false,
                },
                &context,
            );
            if [20, 40, 60, 80, 100, 119].contains(&frame) {
                for draw_frame in 0..3 {
                    animation_camera_bounds("rope", vec3(-45., -45., -10.), vec3(45., 45., 100.));
                    draw_line_3d(vec3(10., 0., -20.), vec3(10., 0., 120.), BROWN);
                    alice.draw(p.feet, true);
                    if draw_frame == 2 {
                        crate::viewer::save_capture(std::path::Path::new(&format!(
                            "private/movement-rope-{name}-{frame}.png"
                        )))?;
                    }
                    next_frame().await;
                }
            }
        }
    }
    println!("PASS real Alice movement skin/feet, paired rope-stroke renders, paused pose and identical save readbacks");
    Ok(())
}

/// Native regression for power skins, saved transformation poses and weapon-only first person.
pub async fn render_presentation(assets: &mut Assets) -> Result<()> {
    use crate::power_pose::Form;
    use crate::powerups::Kind;
    let floor = World::fixture(&[(vec3(-1000., -1000., -50.), vec3(1000., 1000., 0.))]);
    let wall = World::fixture(&[
        (vec3(-1000., -1000., -50.), vec3(1000., 1000., 0.)),
        (vec3(32., -1000., 0.), vec3(40., 1000., 500.)),
    ]);
    let mut player = Player::new(Vec3::ZERO);
    player.grounded = true;
    let mut alice = Character::load(assets)?;
    let input = |selected, first_person, click| WeaponInput {
        selected,
        first_person,
        click,
        aim: Vec3::X,
        dice: 1,
    };
    for (kind, form, name) in [
        (Kind::Rage, Form::Rage, "rage"),
        (Kind::Tea, Form::Tea, "tea"),
    ] {
        alice.reset(&player, 0.);
        let mut stats = crate::inventory::Stats::for_level("skool1", None);
        anyhow::ensure!(stats.powerup(kind), "Fixture power activation failed");
        let mut frame = 0;
        for (label, target) in [("start", 0.8), ("growing", 4.8), ("active", 14.)] {
            while (frame as f32) / 60. < target {
                alice.update_funded(
                    1. / 60.,
                    &player,
                    false,
                    input(0, false, None),
                    &crate::combat::Context {
                        world: &floor,
                        targets: &[],
                    },
                    Some(&mut stats),
                );
                stats.update(1. / 60.);
                frame += 1;
            }
            anyhow::ensure!(
                alice.power_pose.state.form == Some(form),
                "Power disappeared"
            );
            visibility_camera(Vec3::ZERO, 1.);
            alice.draw(Vec3::ZERO, true);
            let original = visibility_image();
            original.export_png(&format!("private/presentation-{name}-{label}.png"));
            let snapshot = serde_json::to_vec(&alice.snapshot())?;
            next_frame().await;
            alice.restore(&serde_json::from_slice(&snapshot)?)?;
            alice.power_appearance(&stats);
            visibility_camera(Vec3::ZERO, 1.);
            alice.draw(Vec3::ZERO, true);
            anyhow::ensure!(
                original.bytes == visibility_image().bytes,
                "Saved {name} {label} pixels differ"
            );
            next_frame().await;
            let paused = serde_json::to_vec(&alice.snapshot())?;
            alice.update_funded(
                0.,
                &player,
                false,
                input(0, false, None),
                &crate::combat::Context {
                    world: &floor,
                    targets: &[],
                },
                Some(&mut stats),
            );
            anyhow::ensure!(
                paused == serde_json::to_vec(&alice.snapshot())?,
                "Paused presentation changed"
            );
        }
        stats.update(60.);
        alice.power_appearance(&stats);
        anyhow::ensure!(
            alice.power_pose.state.form.is_none(),
            "Expired skin remained"
        );
        visibility_camera(Vec3::ZERO, 1.);
        alice.draw(Vec3::ZERO, true);
        visibility_image().export_png(&format!("private/presentation-{name}-expired.png"));
        next_frame().await;
        stats.powerup(kind);
        alice.power_appearance(&stats);
        alice.die();
        anyhow::ensure!(
            alice.power_pose.state.form.is_none(),
            "Power survived death"
        );
        println!("PASS native {name} growth, alternate skin, save pixel equality, pause, expiry and death");
    }
    for selected in 0..10 {
        alice.reset(&player, 0.);
        for _ in 0..100 {
            alice.update(
                1. / 60.,
                &player,
                false,
                input(selected, true, None),
                &crate::combat::Context {
                    world: &floor,
                    targets: &[],
                },
            );
        }
        for (label, world, click) in [
            ("idle", &floor, None),
            ("attack", &floor, Some(false)),
            ("wall", &wall, Some(false)),
        ] {
            for _ in 0..6 {
                alice.update(
                    1. / 60.,
                    &player,
                    false,
                    input(selected, true, click),
                    &crate::combat::Context {
                        world,
                        targets: &[],
                    },
                );
            }
            clear_background(Color::new(0.12, 0.14, 0.18, 1.));
            let camera = Camera3D {
                position: player.eye(),
                target: player.eye() + Vec3::X,
                up: Vec3::Z,
                fovy: 75_f32.to_radians(),
                z_near: 2.,
                z_far: 1000.,
                ..Default::default()
            };
            set_camera(&camera);
            if label == "wall" {
                draw_cube(vec3(34., 0., 70.), vec3(4., 180., 140.), None, GRAY);
            }
            let before = get_screen_data();
            crate::render::clear_view_depth();
            alice.draw_first_person(true);
            let rendered = get_screen_data();
            anyhow::ensure!(
                visible_pixels(&before, &rendered) > 50,
                "Invisible first-person pose {selected}/{label}"
            );
            rendered.export_png(&format!("private/presentation-fp-{selected}-{label}.png"));
            next_frame().await;
            let saved = serde_json::to_vec(&alice.snapshot())?;
            alice.restore(&serde_json::from_slice(&saved)?)?;
            anyhow::ensure!(
                saved == serde_json::to_vec(&alice.snapshot())?,
                "First-person restoration differs"
            );
        }
    }
    alice.check_visible("power-up presentation").await?;
    println!("PASS native weapon-only first person for ten toys, switch/attack/wall poses and restored view placement");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skeletal::Frame;
    fn animator() -> Animator {
        Animator::new(
            (0..CLIPS.len())
                .map(|_| Animation {
                    frames: (0..20)
                        .map(|_| Frame {
                            delta: Vec3::X * 5.,
                            pose: vec![Transform {
                                rotation: Quat::IDENTITY,
                                translation: Vec3::ZERO,
                            }],
                            min: Vec3::ZERO,
                            max: Vec3::ONE,
                        })
                        .collect(),
                    frame_time: 0.05,
                    distance: 100.,
                })
                .collect(),
        )
    }
    #[test]
    fn walking_cadence_tracks_resolved_speed_in_every_equipment_pose() {
        for (unarmed, large, index) in [(true, false, 1), (false, false, 7), (false, true, 9)] {
            for fraction in [0.05, 0.125, 0.25, 0.5, 1., 2.] {
                let mut a = animator();
                a.unarmed = unarmed;
                a.large = large;
                // A one-second walk with the short root travel from issue #2.
                a.clips[index].distance = 51.7;
                let mut p = Player::new(Vec3::ZERO);
                p.grounded = true;
                p.velocity = Vec3::X * crate::movement::WALK_SPEED * fraction;
                a.reset(&p);
                let mut elapsed = 0.;
                for _ in 0..120 {
                    p.feet += p.velocity / 120.;
                    a.update(1. / 120., &p, false);
                    assert_eq!(a.sound_clip, Some(index));
                    let span = a.sound_span.unwrap().1;
                    elapsed += span.end - span.start;
                }
                assert!(
                    (elapsed / fraction - 1.).abs() < 0.02,
                    "{} at {fraction} speed advanced {elapsed} seconds",
                    CLIPS[index]
                );
                // Requested movement against a wall must not keep the legs stepping.
                a.update(1. / 120., &p, false);
                assert_eq!(a.motion, Motion::Idle);
            }
        }
    }
    #[test]
    fn walking_equipment_and_run_changes_preserve_stride_and_save_continuation() {
        let mut a = animator();
        a.unarmed = true;
        a.clips[1].distance = 51.7;
        a.clips[9].frame_time = 0.04;
        let mut p = Player::new(Vec3::ZERO);
        p.grounded = true;
        p.velocity = Vec3::X * crate::movement::WALK_SPEED;
        a.reset(&p);
        p.feet.x = a.clips[1].distance * 0.25;
        a.update(0.25, &p, false);
        assert!((a.time - 0.25).abs() < 0.001);
        a.unarmed = false;
        a.large = true;
        // A render-only frame switches equipment without consuming another step.
        a.update(1. / 144., &p, false);
        assert!((a.time / a.duration(9) - 0.25).abs() < 0.001);
        assert!(!a.sound_span.unwrap().1.entered);
        a.update(1. / 144., &p, true);
        assert!((a.time / a.duration(10) - 0.25).abs() < 0.001);
        p.feet.x += 10.;
        a.update(0.05, &p, true);
        // Running still uses authored distance: this fixture travels 100 units/cycle.
        assert!((a.time - 0.35).abs() < 0.001);
        a.update(1. / 144., &p, false);
        assert!((a.time / a.duration(9) - 0.35).abs() < 0.001);
        let saved = a.snapshot();
        a.update(0., &p, false);
        assert!(a.sound_span.is_none());
        assert_eq!(a.time, saved.time);
        p.feet.x += 10.;
        a.update(0.05, &p, false);
        let expected = serde_json::to_vec(&a.snapshot()).unwrap();
        a.restore(&saved);
        a.update(0.05, &p, false);
        assert_eq!(expected, serde_json::to_vec(&a.snapshot()).unwrap());
    }
    #[test]
    fn rope_cycle_uses_both_hands_and_only_distance_along_rope() {
        let mut a = animator();
        for i in [22, 23, 30, 31] {
            for (j, frame) in a.clips[i].frames.iter_mut().enumerate() {
                // Stationary root frames must not skip the hand-placement pose.
                frame.delta = if j % 2 == 0 { Vec3::ZERO } else { Vec3::Z * 5. };
                frame.pose[0].translation.x = if i < 30 { j as f32 } else { 20. - j as f32 };
            }
        }
        let mut p = Player::new(Vec3::ZERO);
        p.script_motion = 1;
        p.script_rope_length = Some(250.);
        p.script_rope_rise = 1.;
        a.reset(&p);
        for _ in 0..150 {
            p.script_rope_length = p.script_rope_length.map(|l| l - 0.5);
            // A swing can cancel the climb's world-Z movement completely.
            a.update(1. / 120., &p, false);
        }
        assert!((a.time - 1.5).abs() < 0.001);
        assert!((a.pose[0].translation.x - 10.).abs() < 0.01);
        let phase = a.time;
        p.feet.z += 20.;
        a.update(1. / 144., &p, false);
        assert_eq!(a.time, phase, "swinging advanced a climb stroke");
        let saved = a.snapshot();
        p.script_rope_length = Some(174.);
        a.update(1. / 120., &p, false);
        let expected = serde_json::to_vec(&a.snapshot()).unwrap();
        a.restore(&saved);
        a.update(1. / 120., &p, false);
        assert_eq!(serde_json::to_vec(&a.snapshot()).unwrap(), expected);
        let spans = a.audio_spans(
            22,
            crate::audio::events::Span {
                start: 0.9,
                end: 2.1,
                duration: 2.,
                frame_time: 0.05,
                looping: true,
                entered: false,
            },
        );
        assert_eq!(
            spans.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            vec![22, 30, 22]
        );
    }
    #[test]
    fn platform_transport_does_not_start_a_walking_gait() {
        let mut a = animator();
        let mut p = Player::new(Vec3::ZERO);
        p.grounded = true;
        a.reset(&p);
        for _ in 0..120 {
            p.feet += vec3(1., 0., 0.2);
            a.update(1. / 120., &p, false);
            assert_eq!(a.motion, Motion::Idle);
        }
        p.velocity = Vec3::X * 100.;
        p.feet += Vec3::X;
        a.update(0.01, &p, false);
        assert_eq!(a.motion, Motion::Walk);
    }
    #[test]
    fn equipment_selects_unarmed_small_and_large_movement_and_water_poses() {
        let mut a = animator();
        let mut p = Player::new(Vec3::ZERO);
        p.grounded = true;
        for (unarmed, large, idle, walk, run, tread) in [
            (true, false, 0, 1, 2, 25),
            (false, false, 6, 7, 8, 12),
            (false, true, 24, 9, 10, 12),
        ] {
            a.unarmed = unarmed;
            a.large = large;
            p.swimming = false;
            p.velocity = Vec3::ZERO;
            a.update(0.1, &p, false);
            assert_eq!(a.sound_clip, Some(idle));
            p.velocity = Vec3::X * 100.;
            p.feet += p.velocity * 0.1;
            a.update(0.1, &p, false);
            assert_eq!(a.sound_clip, Some(walk));
            p.feet += p.velocity * 0.1;
            a.update(0.1, &p, true);
            assert_eq!(a.sound_clip, Some(run));
            p.swimming = true;
            p.velocity = Vec3::ZERO;
            a.update(0.1, &p, false);
            assert_eq!(a.sound_clip, Some(tread));
        }
    }
    #[test]
    fn animation_sound_span_resumes_without_replaying_entry_or_paused_frames() {
        let mut a = animator();
        let mut p = Player::new(Vec3::ZERO);
        p.grounded = true;
        p.velocity = Vec3::X * 100.;
        a.update(0.2, &p, true);
        assert!(a.sound_span.unwrap().1.entered);
        let saved = a.snapshot();
        a.update(0., &p, true);
        assert!(a.sound_span.is_none());
        a.restore(&saved);
        p.feet += p.velocity * 0.1;
        a.update(0.1, &p, true);
        let (_, span) = a.sound_span.unwrap();
        assert!(!span.entered);
        assert_eq!(span.start, saved.time);
        assert!(span.end > span.start);
    }
    #[test]
    fn locomotion_jumping_landing_pause_and_reset() {
        let mut a = animator();
        let mut p = Player::new(Vec3::ZERO);
        p.grounded = true;
        a.update(0.1, &p, false);
        assert_eq!(a.motion, Motion::Idle);
        p.velocity = Vec3::X * 210.;
        p.feet += p.velocity * 0.1;
        a.update(0.1, &p, false);
        assert_eq!(a.motion, Motion::Walk);
        p.feet += p.velocity * 0.1;
        a.update(0.1, &p, true);
        assert_eq!(a.motion, Motion::Run);
        p.grounded = false;
        p.jumps += 1;
        a.update(0.1, &p, true);
        assert_eq!(a.motion, Motion::Takeoff);
        for _ in 0..12 {
            a.update(0.1, &p, true);
        }
        assert_eq!(a.motion, Motion::Air);
        let time = a.time;
        a.update(0., &p, true);
        assert_eq!(a.time, time);
        p.velocity = Vec3::ZERO;
        p.grounded = true;
        p.landings += 1;
        a.update(0.1, &p, false);
        assert_eq!(a.motion, Motion::Land);
        for _ in 0..8 {
            a.update(0.1, &p, false);
        }
        assert_eq!(a.motion, Motion::Idle);
        a.reset(&p);
        a.update(0.1, &p, false);
        assert_eq!(a.motion, Motion::Idle);
    }
    #[test]
    fn following_camera_stops_before_walls_and_hides_skin_when_cramped() {
        let wall = World::fixture(&[(vec3(-60., -100., -100.), vec3(-50., 100., 100.))]);
        let (camera, target, visible) = follow_camera(&wall, Vec3::ZERO, Vec3::X);
        assert!(camera.x > -46. && camera.x < -45. && visible);
        assert!(!wall.sweep(camera, camera, Vec3::splat(3.9)).start_solid);
        assert!(target.distance(camera) > 1.);
        let tight = World::fixture(&[(vec3(-60., -100., -100.), vec3(-20., 100., 100.))]);
        let (_, _, visible) = follow_camera(&tight, Vec3::ZERO, Vec3::X);
        assert!(!visible);
    }
    #[test]
    fn swimming_uses_full_body_clips_and_pause_preserves_pose() {
        let mut a = animator();
        let mut p = Player::new(Vec3::ZERO);
        p.swimming = true;
        a.update(0.1, &p, false);
        assert_eq!(a.motion, Motion::Tread);
        p.velocity = Vec3::Z * 150.;
        a.update(0.1, &p, false);
        assert_eq!(a.motion, Motion::Swim);
        a.update(0.1, &p, false);
        assert_eq!(a.offset, 24.);
        let time = a.time;
        a.update(0., &p, false);
        assert_eq!(a.time, time);
        p.swimming = false;
        p.grounded = true;
        p.velocity = Vec3::ZERO;
        a.update(0.1, &p, false);
        assert_eq!(a.motion, Motion::Idle);
    }
    #[test]
    fn death_finishes_once_and_respawn_clears_the_pose() {
        let mut a = animator();
        let p = Player::new(Vec3::ZERO);
        a.motion = Motion::Death;
        for _ in 0..40 {
            a.update(0.1, &p, false);
        }
        assert_eq!(a.motion, Motion::Death);
        assert_eq!(a.time, a.clips[16].duration());
        a.reset(&p);
        assert_eq!(a.motion, Motion::Idle);
        assert_eq!(a.time, 0.);
        assert_eq!(a.offset, 0.);
    }

    // Saved Alice blocks. The window-free helpers below stand in for `Character::restore`
    // followed by `power_appearance` on a rig with the fixture's bone count: they run the same
    // validation and the real `Actions::restore`, then the same upgrades the character applies.
    fn stock_transform() -> Transform {
        Transform {
            rotation: Quat::IDENTITY,
            translation: Vec3::ZERO,
        }
    }
    /// A block as written before the power-up, footing, idle-performance, facial-clock and
    /// foot-plant state existed: none of those fields are present.
    fn legacy_block(bones: usize) -> Snapshot {
        let pose = vec![stock_transform(); bones];
        serde_json::from_value(serde_json::json!({
            "animator": {
                "motion": "Idle", "time": 0.25, "transition": 1.0, "previous": pose, "pose": pose,
                "jumps": 3, "landings": 2, "large": false, "offset": 0.0, "previous_offset": 0.0
            },
            "facing": 0.5,
            "actions": crate::weapons::test_actions_with_bones(bones).snapshot(),
            "projectiles": {"projectiles": [], "shots": 0, "impacts": 0},
            "anchors": [stock_transform(), stock_transform(), stock_transform()],
            "viewmodel": crate::viewmodel::ViewModel::default()
        }))
        .unwrap()
    }
    fn restore_headless(old: &Snapshot, stats: &crate::inventory::Stats) -> Result<Snapshot> {
        let bones = old.animator.pose.len();
        old.restored_headless(
            bones,
            &mut crate::weapons::test_actions_with_bones(bones),
            stats,
        )
    }
    fn saved(s: &Snapshot) -> serde_json::Value {
        serde_json::to_value(s).unwrap()
    }
    fn changed_keys(a: &Snapshot, b: &Snapshot) -> Vec<String> {
        let (a, b) = (saved(a), saved(b));
        let mut keys: Vec<_> = a
            .as_object()
            .unwrap()
            .keys()
            .chain(b.as_object().unwrap().keys())
            .filter(|k| a[k.as_str()] != b[k.as_str()])
            .cloned()
            .collect();
        keys.sort();
        keys.dedup();
        keys
    }
    #[test]
    fn legacy_alice_block_restores_unchanged_except_for_the_power_presentation() {
        let old = legacy_block(4);
        let stats = crate::inventory::Stats::default();
        let restored = restore_headless(&old, &stats).unwrap();
        // The reproduced failure: the block written without a power-up presentation gains an
        // initialised one, so a comparison against the raw old block can never pass.
        assert_eq!(changed_keys(&old, &restored), ["power"]);
        assert_eq!(saved(&old)["power"]["initialized"], false);
        assert_eq!(saved(&restored)["power"]["initialized"], true);
        assert_eq!(restored.power.form, None);
        assert!(!restored.power.performing);
        assert!(
            restored.power.age >= 2.5,
            "an existing effect is not replayed"
        );
        // The documented migration predicts the restore exactly, and a current block is a fixed point.
        assert_eq!(saved(&old.migrated(&stats)), saved(&restored));
        let again = restore_headless(&restored, &stats).unwrap();
        assert!(changed_keys(&restored, &again).is_empty());
    }
    #[test]
    fn migrated_alice_block_predicts_forms_deaths_and_unequipped_restores() {
        let old = legacy_block(4);
        let mut rage = crate::inventory::Stats::default();
        rage.powers.rage = 10.;
        let mut tea = crate::inventory::Stats::default();
        tea.powers.tea = 10.;
        let mut dead = crate::inventory::Stats::default();
        dead.damage(1000.);
        let mut empty_handed = serde_json::to_value(crate::inventory::Stats::default()).unwrap();
        empty_handed["owned"] = serde_json::json!(vec![0u8; 10]);
        let mut empty_handed: crate::inventory::Stats =
            serde_json::from_value(empty_handed).unwrap();
        empty_handed.powers.rage = 5.;
        for (stats, form, armed) in [
            (rage, Some(crate::power_pose::Form::Rage), true),
            (tea, Some(crate::power_pose::Form::Tea), true),
            (dead, None, true),
            (empty_handed, Some(crate::power_pose::Form::Rage), false),
        ] {
            assert_eq!(power_form(&stats), form);
            let restored = restore_headless(&old, &stats).unwrap();
            assert_eq!(restored.power.form, form);
            // A legacy save applies its active effect without repeating the pickup performance.
            assert!(!restored.power.performing && restored.power.age >= 2.5);
            assert_eq!(
                saved(&old.migrated(&stats)),
                saved(&restored),
                "forms, deaths and an empty inventory"
            );
            assert_eq!(
                saved(&restored)["actions"]["selected"] == crate::weapons::UNARMED,
                !armed
            );
        }
    }
    /// Every retained legacy fixture that carries an Alice block. The fixtures are private
    /// local files: a missing one is skipped, exactly like `save_check` skips them.
    const LEGACY_FIXTURES: [&str; 15] = [
        "private/event-legacy-v1-school.json",
        "private/event-legacy-v1-battle.json",
        "private/dice-legacy-v2-school.json",
        "private/dice-legacy-v2-battle.json",
        "private/ladybug-legacy-v3/quick.json",
        "private/ladybug-legacy-v3/auto.json",
        "private/pand-legacy-v4/quick.json",
        "private/pand-legacy-v4/auto.json",
        "private/return-legacy-v5/quick.json",
        "private/cinema-legacy-v6/quick.json",
        "private/duchess-legacy-v7/quick.json",
        "private/duchess-legacy-v7/auto.json",
        "private/duchess-native-reward/quick.json",
        "private/duchess-native-reward/auto.json",
        "private/duchess-native-reward/auto.previous.json",
    ];
    #[test]
    fn retained_legacy_fixtures_restore_to_their_migrated_alice_blocks() {
        let mut checked = 0;
        for path in LEGACY_FIXTURES {
            let Ok(bytes) = std::fs::read(path) else {
                eprintln!("skipped {path}: private fixture not present");
                continue;
            };
            let envelope: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let payload = &envelope["payload"];
            let old: Snapshot = serde_json::from_value(payload["character"].clone())
                .unwrap_or_else(|e| panic!("{path}: {e}"));
            let stats: crate::inventory::Stats = serde_json::from_value(payload["stats"].clone())
                .unwrap_or_else(|e| panic!("{path}: {e}"));
            let restored = restore_headless(&old, &stats).unwrap_or_else(|e| panic!("{path}: {e}"));
            let had_power = payload["character"].get("power").is_some();
            let changed = changed_keys(&old, &restored);
            assert!(
                changed
                    == if had_power {
                        vec![]
                    } else {
                        vec!["power".to_string()]
                    },
                "{path}: restore rewrote {changed:?}"
            );
            assert_eq!(
                saved(&old.migrated(&stats)),
                saved(&restored),
                "{path}: migrated prediction"
            );
            let again = restore_headless(&restored, &stats).unwrap();
            assert!(
                changed_keys(&restored, &again).is_empty(),
                "{path}: a current-format block must restore unchanged"
            );
            checked += 1;
        }
        eprintln!(
            "legacy Alice blocks checked: {checked} of {}",
            LEGACY_FIXTURES.len()
        );
    }
}

struct Burn {
    clip: Animation,
    time: f32,
    weight: f32,
    scream: bool,
    was_burning: bool,
}
impl Burn {
    /// Overlay the looping `burn` torso clip on the upper body while Alice is on fire.
    fn apply(&mut self, dt: f32, burning: bool, pose: &mut [Transform], upper: &[bool]) {
        if burning && !self.was_burning {
            self.time = 0.;
            self.scream = true;
        }
        self.was_burning = burning;
        let target = if burning { 1. } else { 0. };
        self.weight += (target - self.weight).clamp(-dt / 0.2, dt / 0.15);
        if burning {
            self.time += dt;
        }
        if self.weight <= 0. {
            return;
        }
        for (i, sample) in self.clip.sample(self.time, true).into_iter().enumerate() {
            if upper.get(i).copied().unwrap_or(false) {
                pose[i] = pose[i].blend(sample, self.weight);
            }
        }
    }
}

/// Procedural two-handed carry and overhead slam for a long Mallet mesh (the Madness Returns
/// hobby horse). Enabled only by a data pack that ships `models/weapons/mallet/twohand.txt`:
/// shaft origin (x y z) and direction (x y z, towards the head) in weapon space, then the signed
/// range along the shaft where the off hand may grip. The weapon's muzzle is weapon +Y.
///
/// Both arms are posed every frame from keyed weapon placements in a chest frame (origin between
/// the shoulders, +X forward, +Y left, +Z up the spine). The weapon hangs from `tag_weapon`, so
/// the right hand is solved from the keyed weapon placement; the left palm (`tag_ball_linked`)
/// is put on the shaft with its knuckles along it. The stock one-handed mallet is untouched.
struct TwoHand {
    left: [usize; 3],
    right: [usize; 3],
    left_palm: usize,
    left_knuckles: [usize; 2],
    spine: usize,
    root: usize,
    legs: [[usize; 3]; 2],
    origin: Vec3,
    direction: Vec3,
    muzzle: Vec3,
    range: (f32, f32),
    weight: f32,
}

/// One keyed weapon placement in the chest frame: where the right palm holds the shaft, the shaft
/// direction (butt to head) and where the horse's face points.
#[derive(Clone, Copy)]
struct GripKey {
    hand: Vec3,
    shaft: Vec3,
    face: Vec3,
}
impl GripKey {
    fn rotation(self) -> Quat {
        let s = self.shaft.normalize();
        let f = (self.face - s * s.dot(self.face)).normalize();
        Quat::from_mat3(&macroquad::math::Mat3::from_cols(s, f, s.cross(f)))
    }
}
/// Madness Returns idle: shaft across the hips, head out on Alice's right at waist height, face
/// down and out, right hand near the head, left hand towards the butt by the left hip.
const HH_CARRY: GripKey = GripKey { hand: vec3(9., -9., -14.), shaft: vec3(0.25, -0.95, 0.12), face: vec3(0.1, -0.2, -1.) };
/// Wind-up: brought up over the right shoulder, head up and back.
const HH_LIFT: GripKey = GripKey { hand: vec3(4., -9., 2.), shaft: vec3(-0.35, -0.25, 0.9), face: vec3(1., 0., 0.3) };
/// Top of the swing: right arm up, horse vertical above her, face forward.
const HH_RAISE: GripKey = GripKey { hand: vec3(3., -5., 17.), shaft: vec3(-0.25, 0.05, 1.), face: vec3(1., 0., 0.) };
/// Impact: slammed down in front of her feet, shaft near vertical, hands low at the chest.
const HH_IMPACT: GripKey = GripKey { hand: vec3(12., -3., -12.), shaft: vec3(0.25, 0.05, -1.), face: vec3(-1., 0., -0.3) };
/// Recovery: lifted off the floor and swung back out to the right side.
const HH_RECOVER: GripKey = GripKey { hand: vec3(12., -8., -12.), shaft: vec3(0.6, -0.65, -0.3), face: vec3(0.2, 0., -1.) };
/// Forward bend of the spine at impact.
const HH_BEND: f32 = 0.2;
/// How far the hips drop (knees bend, feet stay planted) at impact.
const HH_CROUCH: f32 = 10.;
/// Share of a leap the feet rise with the hips (the rest tucks the knees).
const HH_TUCK: f32 = 0.75;
/// Spin: how far she twists right to wind up, and the turn at which the horse passes in front.
const HH_SPIN_WIND: f32 = 0.6;
const HH_SPIN_FRONT: f32 = 0.8;
/// Spin wind-up: horse cocked back low on her right.
const HH_WIND: GripKey = GripKey { hand: vec3(2., -12., -6.), shaft: vec3(-0.6, -0.75, 0.1), face: vec3(0.6, -0.5, 0.) };
/// Spin sweep: horse held straight out to her front-right at waist height, face leading.
const HH_SWEEP: GripKey = GripKey { hand: vec3(14., -8., -6.), shaft: vec3(0.55, -0.83, 0.05), face: vec3(0.8, 0.5, 0.) };
/// Combo uppercut: horse low behind her right hip, head down.
const HH_UC_LOW: GripKey = GripKey { hand: vec3(-1., -11., -15.), shaft: vec3(-0.45, -0.35, -0.82), face: vec3(0., -1., 0.) };
/// Uppercut rising through the front, head leading forward.
const HH_UC_FRONT: GripKey = GripKey { hand: vec3(14., -6., -9.), shaft: vec3(0.85, -0.1, -0.5), face: vec3(0.5, 0., 0.85) };
/// Top of the uppercut: horse high overhead.
const HH_UC_HIGH: GripKey = GripKey { hand: vec3(5., -4., 16.), shaft: vec3(0.35, 0., 0.94), face: vec3(-1., 0., 0.35) };
/// Brought down low on her left after the uppercut.
const HH_SP_LEFT: GripKey = GripKey { hand: vec3(8., 2., -13.), shaft: vec3(0.25, 0.9, -0.35), face: vec3(0., 0., -1.) };
/// Spin: held out to her front-left at waist height, face leading the turn.
const HH_SP_SWEEP: GripKey = GripKey { hand: vec3(13., 3., -6.), shaft: vec3(0.55, 0.83, 0.05), face: vec3(0.83, -0.55, 0.) };
/// End of the spin: raised at her left shoulder.
const HH_SP_END: GripKey = GripKey { hand: vec3(3., 0., 10.), shaft: vec3(0.1, 0.45, 0.9), face: vec3(1., 0., 0.) };
/// Head planted on the floor in front to vault from.
const HH_PLANT: GripKey = GripKey { hand: vec3(8., -4., -16.), shaft: vec3(0.1, 0., -1.), face: vec3(1., 0., 0.) };
/// Springing up off the planted horse, arm pushing down.
const HH_VAULT: GripKey = GripKey { hand: vec3(4., -5., -17.), shaft: vec3(0.1, 0., -1.), face: vec3(1., 0., 0.) };
/// Airborne: raised high in the right hand alone, cocked back.
const HH_ONE_HIGH: GripKey = GripKey { hand: vec3(-1., -6., 18.), shaft: vec3(-0.45, 0., 0.9), face: vec3(1., 0., 0.5) };
/// One-handed slam: head driven into the floor ahead of her right side.
const HH_ONE_SLAM: GripKey = GripKey { hand: vec3(12., -6., -11.), shaft: vec3(0.35, 0., -0.94), face: vec3(-0.94, 0., -0.35) };
/// Where the free left hand reaches for balance in one-handed swings (out to her side).
const HH_BALANCE: Vec3 = vec3(-2., 24., 3.);
const HH_RIGHT_POLE: Vec3 = vec3(-0.4, -1., -0.5);
const HH_LEFT_POLE: Vec3 = vec3(-0.4, 1., -0.5);

/// Body motion that goes with a beat: how far the hips drop (negative: rise, a jump), how far
/// the spine leans forward (radians, negative arches back) and how far the whole body has turned
/// about the hips (radians, positive to her left).
#[derive(Clone, Copy, Default)]
struct HhBody {
    drop: f32,
    bend: f32,
    turn: f32,
    /// How far the left hand has let go of the shaft (one-handed swings).
    free: f32,
}
impl HhBody {
    const fn new(drop: f32, bend: f32) -> Self {
        Self { drop, bend, turn: 0., free: 0. }
    }
    const fn full(drop: f32, bend: f32, turn: f32, free: f32) -> Self {
        Self { drop, bend, turn, free }
    }
    const fn turned(self, turn: f32) -> Self {
        Self { turn, ..self }
    }
    fn lerp(self, other: Self, u: f32) -> Self {
        Self {
            drop: self.drop + (other.drop - self.drop) * u,
            bend: self.bend + (other.bend - self.bend) * u,
            turn: self.turn + (other.turn - self.turn) * u,
            free: self.free + (other.free - self.free) * u,
        }
    }
}
/// Which Madness Returns swing an attack plays.
#[derive(Clone, Copy, PartialEq, Debug)]
enum HhSwing {
    /// Overhead slam (first primary swing).
    Slam,
    /// Spinning sweep (second primary swing).
    Spin,
    /// Uppercut, spin and vault into a one-handed slam (alternate attack).
    Combo,
}
/// One beat of a swing: blend from one placement to the next.
struct HhBeat {
    from: GripKey,
    to: GripKey,
    u: f32,
    body: HhBody,
}
fn hh_smooth(x: f32) -> f32 {
    let x = x.clamp(0., 1.);
    x * x * (3. - 2. * x)
}
/// Recovery shared by both swings: lift off the floor, swing out to the right, back to the carry.
fn hh_recover(from: GripKey, body: HhBody, x: f32) -> HhBeat {
    if x < 0.4 {
        let u = hh_smooth(x / 0.4);
        HhBeat { from, to: HH_RECOVER, u, body: body.lerp(HhBody::default(), u * 0.6) }
    } else {
        let u = hh_smooth((x - 0.4) / 0.6);
        HhBeat { from: HH_RECOVER, to: HH_CARRY, u, body: body.lerp(HhBody::default(), 0.6 + 0.4 * u) }
    }
}
/// Madness Returns overhead slam (primary) for an attack at `time` of `duration` whose hit lands
/// at `hit`.
fn hh_slam(time: f32, duration: f32, hit: f32) -> HhBeat {
    let hit = hit.clamp(0.15, duration.max(0.2));
    let lift = hit * 0.30;
    let raise = hit * 0.60;
    let recover = 0.25f32.min((duration - hit) * 0.5).max(0.);
    let hold = (duration - recover).max(hit);
    let down = HhBody::new(HH_CROUCH, HH_BEND);
    if time < lift {
        HhBeat { from: HH_CARRY, to: HH_LIFT, u: hh_smooth(time / lift), body: HhBody::default() }
    } else if time < raise {
        HhBeat { from: HH_LIFT, to: HH_RAISE, u: hh_smooth((time - lift) / (raise - lift)), body: HhBody::default() }
    } else if time < hit {
        let x = ((time - raise) / (hit - raise)).clamp(0., 1.);
        HhBeat { from: HH_RAISE, to: HH_IMPACT, u: x * x, body: HhBody::default().lerp(down, x * x) }
    } else if time < hold || recover <= 0. {
        HhBeat { from: HH_IMPACT, to: HH_IMPACT, u: 1., body: down }
    } else {
        hh_recover(HH_IMPACT, down, (time - hold) / recover)
    }
}
#[derive(Clone, Copy)]
enum HhEase {
    Smooth,
    In,
    Out,
}
/// One key of a keyed swing: at `at` seconds the weapon sits at `key` with `body`, reached from
/// the previous key with `ease`.
struct HhFrame {
    at: f32,
    key: GripKey,
    body: HhBody,
    ease: HhEase,
}
const fn hh_frame(at: f32, key: GripKey, drop: f32, bend: f32, turn: f32, free: f32, ease: HhEase) -> HhFrame {
    HhFrame { at, key, body: HhBody::full(drop, bend, turn, free), ease }
}
const HH_TURN: f32 = -std::f32::consts::TAU;
/// Madness Returns hobby-horse combo (alternate), keyed from the reference footage. Hits land at
/// `weapons::HOBBY_COMBO_HITS` (0.5 uppercut, 1.2 spin, 2.3 slam) of 3.4 s.
///
/// Uppercut: from low behind her right hip, the horse sweeps forward and up in front of her to
/// high overhead, then drops to her left. Spin: wound to the left, she pivots a full turn to her
/// right with it held out at waist height, finishing raised at her left shoulder. Vault: she
/// plants the head in front, springs up over it, raises it high in her right hand alone with the
/// left arm out for balance, drives it down into the floor and lands kneeling before getting up.
const HH_COMBO: [HhFrame; 15] = [
    hh_frame(0.00, HH_CARRY, 0., 0., 0., 0., HhEase::Smooth),
    hh_frame(0.20, HH_UC_LOW, 4., 0.15, 0., 0., HhEase::Smooth),
    hh_frame(0.40, HH_UC_FRONT, 2., 0.05, 0., 0., HhEase::In),
    hh_frame(0.62, HH_UC_HIGH, -3., -0.2, 0., 0., HhEase::Out),
    hh_frame(0.85, HH_SP_LEFT, 5., 0.15, 0.3, 0., HhEase::Smooth),
    hh_frame(1.00, HH_SP_SWEEP, 4., 0.1, 0.6, 0., HhEase::Smooth),
    hh_frame(1.20, HH_SP_SWEEP, 4., 0.1, -0.93, 0., HhEase::In),
    hh_frame(1.45, HH_SP_SWEEP, 3., 0.05, HH_TURN, 0., HhEase::Out),
    hh_frame(1.62, HH_SP_END, 0., -0.05, HH_TURN, 0., HhEase::Smooth),
    hh_frame(1.80, HH_PLANT, 10., 0.3, HH_TURN, 0., HhEase::Smooth),
    hh_frame(1.98, HH_VAULT, -14., 0.1, HH_TURN, 0., HhEase::Out),
    hh_frame(2.12, HH_ONE_HIGH, -26., -0.2, HH_TURN, 1., HhEase::Out),
    hh_frame(2.30, HH_ONE_SLAM, 16., 0.5, HH_TURN, 1., HhEase::In),
    hh_frame(2.95, HH_ONE_SLAM, 16., 0.5, HH_TURN, 1., HhEase::Smooth),
    hh_frame(3.40, HH_CARRY, 0., 0., HH_TURN, 0., HhEase::Smooth),
];
fn hh_track(frames: &[HhFrame], time: f32) -> HhBeat {
    let i = frames.iter().rposition(|f| f.at <= time).unwrap_or(0);
    let a = &frames[i];
    let Some(b) = frames.get(i + 1) else {
        return HhBeat { from: a.key, to: a.key, u: 1., body: a.body };
    };
    let x = ((time - a.at) / (b.at - a.at)).clamp(0., 1.);
    let u = match b.ease {
        HhEase::Smooth => hh_smooth(x),
        HhEase::In => x * x,
        HhEase::Out => 1. - (1. - x) * (1. - x),
    };
    HhBeat { from: a.key, to: b.key, u, body: a.body.lerp(b.body, u) }
}
fn hh_combo(time: f32, duration: f32) -> HhBeat {
    let last = HH_COMBO[HH_COMBO.len() - 1].at;
    hh_track(&HH_COMBO, time * last / duration.max(1e-3))
}
/// Madness Returns spinning sweep (second primary swing): wind up with the horse cocked back on
/// her right, then pivot a full turn to her left with it held out at waist height, passing in
/// front of her on the hit, and settle back into the carry.
fn hh_spin(time: f32, duration: f32, hit: f32) -> HhBeat {
    let hit = hit.clamp(0.2, duration.max(0.25));
    let wind = hit * 0.43;
    let around = (hit + (duration - hit) * 0.65).min(duration);
    let set = HhBody::new(4., 0.1);
    let wound = set.turned(-HH_SPIN_WIND);
    let front = set.turned(HH_SPIN_FRONT);
    let full = set.turned(std::f32::consts::TAU);
    if time < wind {
        let u = hh_smooth(time / wind);
        HhBeat { from: HH_CARRY, to: HH_WIND, u, body: HhBody::default().lerp(wound, u) }
    } else if time < hit {
        let x = ((time - wind) / (hit - wind)).clamp(0., 1.);
        let u = x * x * x;
        HhBeat { from: HH_WIND, to: HH_SWEEP, u: hh_smooth(x * 1.6), body: wound.lerp(front, u) }
    } else if time < around {
        let x = ((time - hit) / (around - hit)).clamp(0., 1.);
        let u = 1. - (1. - x).powf(1.5);
        HhBeat { from: HH_SWEEP, to: HH_SWEEP, u: 1., body: front.lerp(full, u) }
    } else {
        // A full turn is the starting orientation again.
        let u = hh_smooth((time - around) / (duration - around).max(1e-3));
        HhBeat { from: HH_SWEEP, to: HH_CARRY, u, body: set.lerp(HhBody::default(), u) }
    }
}

impl TwoHand {
    fn load(assets: &mut Assets, skeleton: &Skeleton) -> Option<Self> {
        let text = String::from_utf8(assets.read("models/weapons/mallet/twohand.txt").ok()?).ok()?;
        Self::parse(&text, skeleton)
    }
    fn parse(text: &str, skeleton: &Skeleton) -> Option<Self> {
        let n = text
            .split_whitespace()
            .map(|v| v.parse::<f32>().ok().filter(|v| v.is_finite() && v.abs() < 500.))
            .collect::<Option<Vec<_>>>()?;
        if n.len() != 8 {
            return None;
        }
        let direction = vec3(n[3], n[4], n[5]);
        if direction.length() < 0.5 {
            return None;
        }
        let direction = direction.normalize();
        let muzzle = (Vec3::Y - direction * direction.y).try_normalize()?;
        let find = |n: &str| skeleton.bones.iter().position(|b| b.name == n);
        Some(Self {
            left: [find("Bip01 L UpperArm")?, find("Bip01 L Forearm")?, find("Bip01 L Hand")?],
            right: [find("Bip01 R UpperArm")?, find("Bip01 R Forearm")?, find("Bip01 R Hand")?],
            left_palm: find("tag_ball_linked")?,
            left_knuckles: [find("Bip01 L Finger1")?, find("Bip01 L Finger2")?],
            spine: find("Bip01 Spine1")?,
            root: find("Bip01")?,
            legs: [
                [find("Bip01 L Thigh")?, find("Bip01 L Calf")?, find("Bip01 L Foot")?],
                [find("Bip01 R Thigh")?, find("Bip01 R Calf")?, find("Bip01 R Foot")?],
            ],
            origin: vec3(n[0], n[1], n[2]),
            direction,
            muzzle,
            range: (n[6].min(n[7]), n[6].max(n[7])),
            weight: 0.,
        })
    }
    /// Chest frame of the current pose: origin between the shoulders, +Z straight up, +Y along
    /// the shoulders (left) kept level, +X forward. Staying upright keeps the keys meaningful
    /// while the swing clips and the slam bend the spine.
    fn chest_frame(&self, world: &[Transform]) -> Transform {
        let (l, r) = (world[self.left[0]].translation, world[self.right[0]].translation);
        let y = vec3(l.x - r.x, l.y - r.y, 0.).try_normalize().unwrap_or(Vec3::Y);
        Transform {
            rotation: Quat::from_mat3(&macroquad::math::Mat3::from_cols(y.cross(Vec3::Z), y, Vec3::Z)).normalize(),
            translation: (l + r) * 0.5,
        }
    }
    /// Weapon-tag placement in the chest frame for a keyed blend.
    fn placement(&self, a: GripKey, b: GripKey, u: f32, shoulder: Vec3) -> Transform {
        // Hands travel on arcs around the right shoulder, not straight through the body.
        let (da, db) = (a.hand - shoulder, b.hand - shoulder);
        let dir = Quat::IDENTITY.slerp(Quat::from_rotation_arc(da.normalize(), db.normalize()), u) * da.normalize();
        let hand = shoulder + dir * (da.length() + (db.length() - da.length()) * u);
        let (qa, mut qb) = (a.rotation(), b.rotation());
        if qa.dot(qb) < 0. {
            qb = -qb;
        }
        let weapon = Quat::from_mat3(&macroquad::math::Mat3::from_cols(self.direction, self.muzzle, self.direction.cross(self.muzzle)));
        let rotation = (qa.slerp(qb, u) * weapon.inverse()).normalize();
        Transform { rotation, translation: hand - rotation * self.origin }
    }
    /// The tag transform the weapon should have this frame, in skeleton space, and the spine bend.
    fn target(&self, play: Option<(f32, f32, f32, HhSwing)>, world: &[Transform]) -> (Transform, HhBody) {
        let (beat, facing) = match play {
            Some((time, duration, hit, HhSwing::Slam)) => (hh_slam(time, duration, hit), 0.),
            Some((time, duration, hit, HhSwing::Spin)) => (hh_spin(time, duration, hit), 0.),
            // The combo is keyed to where she faces (the aim), not to the twisted two-handed
            // stance, so the uppercut, spin and slam land in front of her. It blends over from
            // the carry and back.
            Some((time, duration, _, HhSwing::Combo)) => {
                (hh_combo(time, duration), hh_smooth(time / 0.2) * hh_smooth((duration - time) / 0.45))
            }
            None => (HhBeat { from: HH_CARRY, to: HH_CARRY, u: 0., body: HhBody::default() }, 0.),
        };
        let chest = self.chest_frame(world);
        let ahead = Quat::from_rotation_z(beat.body.turn * self.weight);
        let frame = Transform { rotation: chest.rotation.slerp(ahead, facing).normalize(), translation: chest.translation };
        let shoulder = frame.rotation.inverse() * (world[self.right[0]].translation - frame.translation);
        let local = self.placement(beat.from, beat.to, beat.u, shoulder);
        (
            Transform {
                rotation: (frame.rotation * local.rotation).normalize(),
                translation: frame.point(local.translation),
            },
            beat.body,
        )
    }
    fn apply(
        &mut self,
        dt: f32,
        held: bool,
        play: Option<(f32, f32, f32, HhSwing)>,
        skeleton: &Skeleton,
        tag: usize,
        pose: &mut [Transform],
    ) {
        let target = if held { 1. } else { 0. };
        self.weight += (target - self.weight).clamp(-dt.max(0.) / 0.15, dt.max(0.) / 0.15);
        if self.weight <= 0. {
            return;
        }
        let w = self.weight;
        let parent_rot = |i: usize, world: &[Transform]| skeleton.bones[i].parent.map_or(Quat::IDENTITY, |p| world[p].rotation);
        // Crouch, leap and lean with the swing. Feet stay planted while the hips drop; in the air
        // they rise with the body and the knees tuck.
        let world = skeleton.global_pose(pose);
        let (_, body) = self.target(play, &world);
        if body.turn != 0. && skeleton.bones[self.root].parent.is_none() {
            pose[self.root].rotation = (Quat::from_rotation_z(body.turn * w) * pose[self.root].rotation).normalize();
        }
        let world = skeleton.global_pose(pose);
        if body.drop != 0. && skeleton.bones[self.root].parent.is_none() {
            let feet = self.legs.map(|l| world[l[2]]);
            let drop = body.drop * w;
            pose[self.root].translation.z -= drop;
            let lift = Vec3::Z * (-drop * HH_TUCK).max(0.);
            for (leg, foot) in self.legs.iter().zip(feet) {
                reach_arm(skeleton, *leg, foot.translation + lift, Some(Vec3::X), 1., pose);
                let world = skeleton.global_pose(pose);
                pose[leg[2]].rotation = (parent_rot(leg[2], &world).inverse() * foot.rotation).normalize();
            }
        }
        let world = skeleton.global_pose(pose);
        if body.bend != 0. {
            let left = self.chest_frame(&world).rotation * Vec3::Y;
            let g = (Quat::from_axis_angle(left, body.bend * w) * world[self.spine].rotation).normalize();
            pose[self.spine].rotation = (parent_rot(self.spine, &world).inverse() * g).normalize();
        }
        let world = skeleton.global_pose(pose);
        let (goal, _) = self.target(play, &world);
        let frame = self.chest_frame(&world).rotation;
        let goal = world[tag].blend(goal, w);
        // Right hand: the weapon hangs from tag_weapon, so solve the hand from the weapon.
        let [_, _, rh] = self.right;
        let rel = Transform {
            rotation: world[rh].rotation.inverse() * world[tag].rotation,
            translation: world[rh].rotation.inverse() * (world[tag].translation - world[rh].translation),
        };
        let hand_rot = (goal.rotation * rel.rotation.inverse()).normalize();
        let wrist = goal.translation - hand_rot * rel.translation;
        reach_arm(skeleton, self.right, wrist, Some(frame * HH_RIGHT_POLE), 1., pose);
        let world = skeleton.global_pose(pose);
        pose[rh].rotation = (parent_rot(rh, &world).inverse() * hand_rot).normalize();
        // Left palm on the shaft, nearest the left shoulder inside the grip range.
        let world = skeleton.global_pose(pose);
        let start = world[tag].point(self.origin);
        let axis = world[tag].rotation * self.direction;
        let along = (world[self.left[0]].translation - start).dot(axis).clamp(self.range.0, self.range.1);
        let grip = start + axis * along;
        let [_, _, lh] = self.left;
        let knuckles = world[self.left_knuckles[0]].translation - world[self.left_knuckles[1]].translation;
        let turn = Quat::from_rotation_arc(knuckles.try_normalize().unwrap_or(axis), axis);
        let left_rot = (turn * world[lh].rotation).normalize();
        let palm = world[lh].rotation.inverse() * (world[self.left_palm].translation - world[lh].translation);
        let hold = w * (1. - body.free.clamp(0., 1.));
        let left_rot = world[lh].rotation.slerp(left_rot, hold).normalize();
        // Let go and take hold again along an arc around the shoulder, not through the body.
        let shoulder = world[self.left[0]].translation;
        let balance = self.chest_frame(&world).point(HH_BALANCE) - shoulder;
        let held_at = grip - left_rot * palm - shoulder;
        let k = (hold / w.max(1e-3)).clamp(0., 1.);
        let arc = Quat::IDENTITY.slerp(Quat::from_rotation_arc(balance.normalize(), held_at.normalize()), k)
            * balance.normalize()
            * (balance.length().min(18.) + (held_at.length() - balance.length().min(18.)) * k);
        let wrist = world[lh].translation.lerp(shoulder + arc, w);
        reach_arm(skeleton, self.left, wrist, Some(frame * HH_LEFT_POLE), 1., pose);
        let world = skeleton.global_pose(pose);
        pose[lh].rotation = (parent_rot(lh, &world).inverse() * left_rot).normalize();
    }
}

/// Two-bone reach of one arm: put the wrist at `goal`, bending the elbow towards `pole`
/// (or the animation's elbow side).
fn reach_arm(skeleton: &Skeleton, bones: [usize; 3], goal: Vec3, pole: Option<Vec3>, weight: f32, pose: &mut [Transform]) {
    let [upper, fore, hand] = bones;
    let world = skeleton.global_pose(pose);
    let shoulder = world[upper].translation;
    let elbow = world[fore].translation;
    let wrist = world[hand].translation;
    let (l1, l2) = ((elbow - shoulder).length(), (wrist - elbow).length());
    let to_goal = goal - shoulder;
    if l1 <= 0. || l2 <= 0. || to_goal.length() < 1e-4 {
        return;
    }
    let d = to_goal.length().clamp((l1 - l2).abs() + 0.01, l1 + l2 - 0.01);
    let axis = to_goal / to_goal.length();
    let side = pole.unwrap_or(elbow - shoulder);
    let mut pole = side - axis * side.dot(axis);
    if pole.length() < 1e-4 {
        pole = (elbow - shoulder) - axis * (elbow - shoulder).dot(axis);
    }
    if pole.length() < 1e-4 {
        pole = Vec3::NEG_Z - axis * Vec3::NEG_Z.dot(axis);
    }
    let pole = pole.normalize();
    let reach = (l1 * l1 - l2 * l2 + d * d) / (2. * d);
    let up = (l1 * l1 - reach * reach).max(0.).sqrt();
    let new_elbow = shoulder + axis * reach + pole * up;
    let swing = |from: Vec3, to: Vec3| Quat::from_rotation_arc(from.normalize(), to.normalize());
    let parent_rot = |i: usize, w: &[Transform]| skeleton.bones[i].parent.map_or(Quat::IDENTITY, |p| w[p].rotation);
    let q1 = swing(elbow - shoulder, new_elbow - shoulder);
    let g1 = (q1 * world[upper].rotation).normalize();
    pose[upper].rotation = pose[upper].rotation.slerp((parent_rot(upper, &world).inverse() * g1).normalize(), weight);
    let world = skeleton.global_pose(pose);
    let elbow2 = world[fore].translation;
    let wrist2 = world[hand].translation;
    let tip = elbow2 + (goal - elbow2).normalize() * l2;
    let q2 = swing(wrist2 - elbow2, tip - elbow2);
    let g2 = (q2 * world[fore].rotation).normalize();
    pose[fore].rotation = pose[fore].rotation.slerp((parent_rot(fore, &world).inverse() * g2).normalize(), weight);
}

#[cfg(test)]
mod hobby_probe {
    use super::*;
    fn dir() -> Option<std::path::PathBuf> {
        std::env::var("HOBBY_DATA").ok().map(std::path::PathBuf::from)
    }
    fn skin(sk: &Skeleton, world: &[Transform]) -> Vec<f32> {
        let mut out = Vec::new();
        for (si, s) in sk.surfaces.iter().enumerate() {
            if ["material15", "material16", "material17"].contains(&s.name.as_str()) {
                continue;
            }
            for v in &s.vertices {
                let p: Vec3 = v.weights.iter().map(|w| world[w.bone].point(w.offset) * w.amount).sum();
                out.extend([p.x, p.y, p.z, si as f32]);
            }
        }
        out
    }
    /// Runs the hobby-horse controller over the real clips and reports contact errors.
    #[test]
    fn controller() {
        let Some(d) = dir() else { return };
        let out = std::path::PathBuf::from(std::env::var("HOBBY_OUT").unwrap());
        std::fs::create_dir_all(&out).unwrap();
        let sk = Skeleton::parse(&std::fs::read(d.join("alice_baseframe.skb")).unwrap()).unwrap();
        let clip = |n: &str| Animation::parse(&std::fs::read(d.join(format!("{n}.ska"))).unwrap(), sk.bones.len()).unwrap();
        let text = std::fs::read_to_string(std::env::var("HOBBY_TWOHAND").unwrap()).unwrap();
        let mut hh = TwoHand::parse(&text, &sk).expect("twohand");
        hh.weight = 1.;
        let tag = sk.bones.iter().position(|b| b.name == "tag_weapon").unwrap();
        let mut report = String::new();
        let mut worst = [0f32; 4];
        // Offline check: HOBBY_DATA=<folder with alice_baseframe.skb and the clips>,
        // HOBBY_TWOHAND=<twohand.txt>, HOBBY_OUT=<dump folder for tools/hobbyhorse_render.py>.
        let mut cases: Vec<(String, Vec<Transform>, Option<(f32, f32, f32, HhSwing)>)> = Vec::new();
        for n in ["ready_bw", "walk_bigwep", "run_bigwep"] {
            let c = clip(n);
            for k in 0..8 {
                let t = c.duration() * k as f32 / 8.;
                cases.push((format!("{n}_{k}"), c.sample(t, true), None));
            }
        }
        for n in ["mallet_att_3_prim_01", "mallet_att_3_prim_02"] {
            let c = clip(n);
            let dur = c.duration();
            report += &format!("{n} duration {dur:.3}\n");
            let swing = if n.ends_with("_02") { HhSwing::Spin } else { HhSwing::Slam };
            let mut t = 0.;
            let mut k = 0;
            while t <= dur + 1e-4 {
                cases.push((format!("{n}_{k:02}"), c.sample(t, false), Some((t, dur, 0.35, swing))));
                t += 1. / 30.;
                k += 1;
            }
        }
        // The combo is posed over the idle stance only.
        let idle = clip("ready_bw");
        let dur = crate::weapons::HOBBY_COMBO_DURATION;
        let mut k = 0;
        while k as f32 / 30. <= dur + 1e-4 {
            let t = k as f32 / 30.;
            cases.push((format!("combo_{k:03}"), idle.sample(0., true), Some((t, dur, 0., HhSwing::Combo))));
            k += 1;
        }
        for (name, mut pose, play) in cases {
            hh.apply(0.0, true, play, &sk, tag, &mut pose);
            let world = sk.global_pose(&pose);
            let (goal, _) = hh.target(play, &world);
            // Right palm: weapon tag vs keyed goal.
            let r_err = (world[tag].translation - goal.translation).length();
            let r_rot = world[tag].rotation.angle_between(goal.rotation).to_degrees();
            // Left palm distance to the shaft line and grip position along it.
            let start = world[tag].point(hh.origin);
            let axis = world[tag].rotation * hh.direction;
            let lp = world[hh.left_palm].translation;
            let along = (lp - start).dot(axis);
            let off = ((lp - start) - axis * along).length();
            let out_of_range = (hh.range.0 - along).max(along - hh.range.1).max(0.);
            let l_err = off + out_of_range;
            let reach_l = (lp - world[hh.left[0]].translation).length();
            worst[0] = worst[0].max(r_err);
            worst[1] = worst[1].max(r_rot);
            worst[2] = worst[2].max(l_err);
            report += &format!(
                "{name}: right {r_err:.2}u {r_rot:.1}deg | left off-shaft {off:.2}u along {along:.1} | left reach {reach_l:.1}\n"
            );
            let mut bytes: Vec<u8> = Vec::new();
            let t = world[tag];
            for v in [t.translation.x, t.translation.y, t.translation.z, t.rotation.x, t.rotation.y, t.rotation.z, t.rotation.w] {
                bytes.extend(v.to_le_bytes());
            }
            for v in skin(&sk, &world) {
                bytes.extend(v.to_le_bytes());
            }
            std::fs::write(out.join(format!("{name}.bin")), bytes).unwrap();
        }
        report += &format!("WORST right {:.2}u {:.1}deg left {:.2}u\n", worst[0], worst[1], worst[2]);
        std::fs::write(out.join("report.txt"), &report).unwrap();
        println!("{report}");
    }
}
