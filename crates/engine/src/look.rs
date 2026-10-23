//! The user-settable cushion look (G-018): ridge height, per-level scale, ambient and diffuse light
//! and the light direction. [`CushionLook`] is the persisted form, [`LookPatch`] is the validated
//! partial update used by `treemap.look` and `settings.set`.
//!
//! Two boundaries, two policies. A command or `settings.set` call is **strict**: a value outside its
//! range is `invalid_params` and changes nothing. A settings file is **lenient**: [`CushionLook::sanitized`]
//! clamps every value into its range and replaces a useless light vector by the default, so a
//! hand-edited or hostile file can never produce a look the renderer cannot handle.

use dirustat_treemap::CushionConfig;
use serde::{Deserialize, Serialize};

/// Largest accepted root ridge height (relative to the tile width).
pub const MAX_HEIGHT: f32 = 4.0;
/// Largest accepted absolute value of a light vector component.
pub const MAX_LIGHT_COMPONENT: f32 = 100.0;
/// Smallest accepted light vector length: a shorter vector has no usable direction.
pub const MIN_LIGHT_LENGTH: f32 = 1e-3;

/// The shortest decimal that reads back as the same `f32` (`0.9`, not `0.8999999761581421`), so the
/// settings file and the control protocol show the numbers the user typed.
fn short(v: f32) -> f64 {
    v.to_string().parse().unwrap_or_else(|_| f64::from(v))
}

fn ser_f32<S: serde::Serializer>(v: &f32, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_f64(short(*v))
}

fn ser_light<S: serde::Serializer>(v: &[f32; 3], s: S) -> Result<S::Ok, S::Error> {
    [short(v[0]), short(v[1]), short(v[2])].serialize(s)
}

/// Persisted cushion look. The defaults are the M5.1 constants (`CushionConfig::default`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CushionLook {
    /// Root ridge height, `0..=4`; `0` is flat.
    #[serde(serialize_with = "ser_f32")]
    pub height: f32,
    /// Ridge height factor per nesting level, `0..=1`.
    #[serde(serialize_with = "ser_f32")]
    pub scale: f32,
    /// Ambient light, `0..=1`.
    #[serde(serialize_with = "ser_f32")]
    pub ambient: f32,
    /// Diffuse light, `0..=1`.
    #[serde(serialize_with = "ser_f32")]
    pub diffuse: f32,
    /// Direction towards the light: x right, y down, z towards the viewer. Components within
    /// `-100..=100` (z within `0..=100`), length at least `1e-3`; it need not be normalised.
    #[serde(serialize_with = "ser_light")]
    pub light: [f32; 3],
}

impl Default for CushionLook {
    fn default() -> Self {
        let c = CushionConfig::default();
        Self { height: c.height, scale: c.scale, ambient: c.ambient, diffuse: c.diffuse, light: c.light }
    }
}

fn light_is_valid(light: [f32; 3]) -> bool {
    let [x, y, z] = light;
    let finite = x.is_finite() && y.is_finite() && z.is_finite();
    finite
        && x.abs() <= MAX_LIGHT_COMPONENT
        && y.abs() <= MAX_LIGHT_COMPONENT
        && (0.0..=MAX_LIGHT_COMPONENT).contains(&z)
        && x.hypot(y).hypot(z) >= MIN_LIGHT_LENGTH
}

impl CushionLook {
    /// The nearest valid look: non-finite values become the default, the rest is clamped.
    pub fn sanitized(self) -> Self {
        let d = Self::default();
        let pick = |v: f32, lo: f32, hi: f32, fallback: f32| if v.is_finite() { v.clamp(lo, hi) } else { fallback };
        let light = {
            let [x, y, z] = self.light;
            let clamped = [
                x.clamp(-MAX_LIGHT_COMPONENT, MAX_LIGHT_COMPONENT),
                y.clamp(-MAX_LIGHT_COMPONENT, MAX_LIGHT_COMPONENT),
                z.clamp(0.0, MAX_LIGHT_COMPONENT),
            ];
            if [x, y, z].iter().all(|v| v.is_finite()) && light_is_valid(clamped) { clamped } else { d.light }
        };
        Self {
            height: pick(self.height, 0.0, MAX_HEIGHT, d.height),
            scale: pick(self.scale, 0.0, 1.0, d.scale),
            ambient: pick(self.ambient, 0.0, 1.0, d.ambient),
            diffuse: pick(self.diffuse, 0.0, 1.0, d.diffuse),
            light,
        }
    }

    /// The renderer's parameters for this look (sanitized first, so the result is always usable).
    pub fn config(self) -> CushionConfig {
        let s = self.sanitized();
        CushionConfig { height: s.height, scale: s.scale, ambient: s.ambient, diffuse: s.diffuse, light: s.light }
    }
}

/// A partial change of the look. Absent fields stay as they are; `reset` restores the defaults
/// first, then applies the other fields.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LookPatch {
    pub reset: bool,
    pub height: Option<f32>,
    pub scale: Option<f32>,
    pub ambient: Option<f32>,
    pub diffuse: Option<f32>,
    pub light: Option<[f32; 3]>,
}

fn in_range(name: &str, v: f32, lo: f32, hi: f32) -> Result<f32, String> {
    if v.is_finite() && (lo..=hi).contains(&v) { Ok(v) } else { Err(format!("{name} must be a number in {lo}..={hi}")) }
}

impl LookPatch {
    /// The look after applying the patch to `current`, or the first violated rule. Nothing is
    /// applied on error.
    pub fn apply(&self, current: CushionLook) -> Result<CushionLook, String> {
        let mut next = if self.reset { CushionLook::default() } else { current };
        if let Some(v) = self.height {
            next.height = in_range("height", v, 0.0, MAX_HEIGHT)?;
        }
        if let Some(v) = self.scale {
            next.scale = in_range("scale", v, 0.0, 1.0)?;
        }
        if let Some(v) = self.ambient {
            next.ambient = in_range("ambient", v, 0.0, 1.0)?;
        }
        if let Some(v) = self.diffuse {
            next.diffuse = in_range("diffuse", v, 0.0, 1.0)?;
        }
        if let Some(light) = self.light {
            if !light_is_valid(light) {
                return Err(format!(
                    "light must be [x, y, z] with |x|, |y| <= {MAX_LIGHT_COMPONENT}, z in 0..={MAX_LIGHT_COMPONENT} \
                     and a length of at least {MIN_LIGHT_LENGTH}"
                ));
            }
            next.light = light;
        }
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_the_renderer_default() {
        assert_eq!(CushionLook::default().config(), CushionConfig::default());
    }

    #[test]
    fn sanitize_clamps_and_replaces_garbage() {
        let nan = f32::NAN;
        let hostile = CushionLook { height: 1e30, scale: -3.0, ambient: nan, diffuse: 7.0, light: [nan, 0.0, 1.0] };
        let s = hostile.sanitized();
        let d = CushionLook::default();
        assert_eq!((s.height, s.scale, s.ambient, s.diffuse), (MAX_HEIGHT, 0.0, d.ambient, 1.0));
        assert_eq!(s.light, d.light, "a non-finite light falls back to the default");
        let zero = CushionLook { light: [0.0; 3], ..d };
        assert_eq!(zero.sanitized().light, d.light);
        let huge = CushionLook { light: [1e9, -1e9, -5.0], ..d }.sanitized();
        assert_eq!(huge.light, [MAX_LIGHT_COMPONENT, -MAX_LIGHT_COMPONENT, 0.0]);
        assert!(light_is_valid(huge.light));
        assert_eq!(d.sanitized(), d, "a valid look is unchanged");
    }

    #[test]
    fn patch_changes_only_given_fields_and_validates() {
        let base = CushionLook::default();
        let p = LookPatch { height: Some(1.0), light: Some([1.0, 0.0, 1.0]), ..LookPatch::default() };
        let next = p.apply(base).unwrap();
        assert_eq!((next.height, next.scale, next.light), (1.0, base.scale, [1.0, 0.0, 1.0]));
        let reset = LookPatch { reset: true, ambient: Some(0.1), ..LookPatch::default() };
        let r = reset.apply(next).unwrap();
        assert_eq!((r.height, r.ambient), (base.height, 0.1));
        for bad in [
            LookPatch { height: Some(4.5), ..LookPatch::default() },
            LookPatch { scale: Some(-0.1), ..LookPatch::default() },
            LookPatch { ambient: Some(f32::NAN), ..LookPatch::default() },
            LookPatch { diffuse: Some(f32::INFINITY), ..LookPatch::default() },
            LookPatch { light: Some([0.0, 0.0, 0.0]), ..LookPatch::default() },
            LookPatch { light: Some([0.0, 0.0, -1.0]), ..LookPatch::default() },
            LookPatch { light: Some([1e6, 0.0, 1.0]), ..LookPatch::default() },
        ] {
            assert!(bad.apply(base).is_err(), "{bad:?}");
        }
    }
}
