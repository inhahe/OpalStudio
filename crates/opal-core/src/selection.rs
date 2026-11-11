//! A selection (design.md, *Selections*, *The recipe*): the steps that made it, in order, and
//! the mask they come to. A document holds one, or none (nothing selected: operations act on
//! whole layers), and so the history keeps it with the rest of each state.

use crate::geom::Affine;
use crate::mask::{Combine, Mask, Shape};
use crate::modify::{Bounds, Falloff, Halt, Reach, Side, UNBOUNDED};
use std::sync::Arc;

/// A step of a selection's recipe: a maker, which makes a mask and combines it with the
/// selection so far, or a modifier, which changes the selection so far.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "step")]
pub enum Step {
    /// A rectangle or an ellipse, combined with the selection so far.
    Shape { shape: Shape, combine: Combine },
    /// The opposite of the selection so far: selected where it was not.
    Invert,
    /// The selection so far grown by `by` pixels (shrunk, by less than 0), its corners as
    /// `corners` says.
    Grow { by: f64, corners: Reach },
    /// The selection so far feathered: its edge falling off over `width` pixels, on the
    /// `side` of its outline, along `falloff`.
    Feather { width: f64, side: Side, falloff: Falloff },
    /// The selection so far with its outline smoothed over `radius` pixels: pinholes and
    /// notches filled, specks and spurs taken away, corners rounded.
    Smooth { radius: f64 },
    /// A band `width` pixels wide along the outline of the selection so far, on its `side`.
    Border { width: f64, side: Side },
    /// A mask as it is, combined with the selection so far as `combine` says, made as `of`
    /// says. It is told without its mask, and so cannot be read back.
    #[serde(skip_deserializing)]
    Mask {
        #[serde(skip)]
        mask: Arc<Mask>,
        combine: Combine,
        of: MaskOf,
    },
}

/// What a mask step's mask was made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskOf {
    /// A layer's pixels, each as much as it is opaque (Select Layer's Pixels).
    LayerPixels,
    /// The steps before it, which a transform their shapes could not follow carried pixel by
    /// pixel (see [`Selection::transformed`]); it replaces what was before it.
    Carried,
    /// A selection saved in the image (Load Selection combining it with what is selected).
    Saved,
}

impl Step {
    /// The selection `before` (its mask) becomes after the step, unless `bounds` hold a
    /// modifier's work back.
    fn apply(&self, before: &Mask, bounds: Bounds) -> Result<Mask, Halt> {
        Ok(match self {
            Step::Shape { shape, combine } => {
                let made = Mask::of(shape);
                before.combined(&made, *combine)
            }
            Step::Invert => before.inverted(),
            Step::Grow { by, corners } => before.grown_within(*by, *corners, bounds)?,
            Step::Feather { width, side, falloff } => before.feathered_within(*width, *side, *falloff, bounds)?,
            Step::Smooth { radius } => before.smoothed_within(*radius, bounds)?,
            Step::Border { width, side } => before.bordered_within(*width, *side, bounds)?,
            Step::Mask { mask, combine, .. } => before.combined(mask, *combine),
        })
    }

    /// Whether it is a modifier (grow, feather, smooth or border): one with settings, which
    /// works the selection so far out again from its outline.
    pub fn is_modifier(&self) -> bool {
        matches!(self, Step::Grow { .. } | Step::Feather { .. } | Step::Smooth { .. } | Step::Border { .. })
    }

    /// Whether it is a modifier at work: one whose mask takes the memory `Bounds` admit as it
    /// is made.
    fn modifies(&self) -> bool {
        self.is_modifier() && !self.leaves_as_is()
    }

    /// Whether the step makes a selection of its own, the steps before it no longer counting.
    fn starts_over(&self) -> bool {
        matches!(self, Step::Shape { combine: Combine::Replace, .. } | Step::Mask { combine: Combine::Replace, .. })
    }

    /// The step `m` carries this one to, if there is one: shapes as [`Shape::transformed`]
    /// says; distances only by a map that scales them alike every way (a turn, a flip, an even
    /// scale), and square or diamond ones only by one that keeps the pixel grid's axes (a
    /// quarter turn, a flip), as they are measured along them; not past
    /// [`MAX_DISTANCE`](crate::modify::MAX_DISTANCE); a mask always.
    fn transformed(&self, m: &Affine) -> Option<Step> {
        let distance = |d: f64, on_axes: bool| {
            let scale = even_scale(m)?;
            let axes = (m.b.abs() < 1e-9 && m.c.abs() < 1e-9) || (m.a.abs() < 1e-9 && m.d.abs() < 1e-9);
            let d = d * scale;
            ((axes || !on_axes) && d.abs() <= crate::modify::MAX_DISTANCE).then_some(d)
        };
        Some(match self {
            Step::Shape { shape, combine } => Step::Shape { shape: shape.transformed(m)?, combine: *combine },
            Step::Invert => Step::Invert,
            Step::Grow { by, corners } => {
                Step::Grow { by: distance(*by, *corners != Reach::Round)?, corners: *corners }
            }
            Step::Feather { width, side, falloff } => {
                Step::Feather { width: distance(*width, false)?, side: *side, falloff: *falloff }
            }
            Step::Smooth { radius } => Step::Smooth { radius: distance(*radius, false)? },
            Step::Border { width, side } => Step::Border { width: distance(*width, false)?, side: *side },
            Step::Mask { mask, combine, of } => {
                Step::Mask { mask: Arc::new(mask.transformed(m)), combine: *combine, of: *of }
            }
        })
    }

    /// Whether it leaves every selection as it is (growing by nothing, a feather or a smooth of
    /// no width).
    fn leaves_as_is(&self) -> bool {
        match self {
            Step::Grow { by, .. } => *by == 0.0,
            Step::Feather { width, .. } | Step::Smooth { radius: width } => *width <= 0.0,
            _ => false,
        }
    }

    /// Whether `m` carries it exactly (without the work of carrying a mask step, which it
    /// always does).
    fn carried_by(&self, m: &Affine) -> bool {
        match self {
            Step::Mask { .. } | Step::Invert => true,
            other => other.transformed(m).is_some(),
        }
    }
}

/// How much `m` scales every length, if it scales them alike every way (its linear part a
/// turn or flip times an even scale).
fn even_scale(m: &Affine) -> Option<f64> {
    let (across, down) = (m.a.hypot(m.b), m.c.hypot(m.d));
    let even =
        (across - down).abs() <= 1e-9 * across.max(down) && (m.a * m.c + m.b * m.d).abs() <= 1e-9 * across * down;
    even.then_some(across)
}

/// A selection: see the module's description. Its mask is worked out from its steps when it
/// is made, and shared by its copies.
#[derive(Clone, Debug)]
pub struct Selection {
    steps: Vec<Step>,
    mask: Arc<Mask>,
    /// The mask its steps but the last make, kept as it was worked out on the way (so that
    /// the box round its last shape need not work them out again), if it was.
    before: Option<Arc<Mask>>,
}

/// Selections are alike if their steps and what they make are: what was kept on the way to it
/// does not count.
impl PartialEq for Selection {
    fn eq(&self, other: &Selection) -> bool {
        self.steps == other.steps && self.mask == other.mask
    }
}

impl Selection {
    /// The selection `steps` make, from nothing selected (the steps before the last that
    /// starts over are left out).
    pub fn of(steps: Vec<Step>) -> Selection {
        Selection::of_within(steps, UNBOUNDED).expect("unbounded")
    }

    /// [`of`](Self::of), unless `bounds` hold a modifier's work back.
    pub fn of_within(steps: Vec<Step>, bounds: Bounds) -> Result<Selection, Halt> {
        let first = steps.iter().rposition(Step::starts_over).unwrap_or(0);
        // One that leaves the selection as it is is no step, as when it is added.
        let steps: Vec<Step> = steps[first..].iter().filter(|s| !s.leaves_as_is()).cloned().collect();
        let (mut mask, mut before) = (Arc::new(Mask::none()), None);
        // Whether the mask before was a modifier's: what it took is given back once it goes.
        let mut made_before = false;
        for (i, s) in steps.iter().enumerate() {
            // A mask step's mask is shared, not copied.
            let next = match s {
                Step::Mask { mask, combine: Combine::Replace, .. } => mask.clone(),
                _ => Arc::new(s.apply(&mask, bounds)?),
            };
            let gone = before.replace(std::mem::replace(&mut mask, next));
            if made_before && let Some(gone) = gone {
                give_back(gone, bounds);
            }
            made_before = i > 0 && steps[i - 1].modifies();
        }
        Ok(Selection { steps, mask, before })
    }

    /// Its mask, the rest of what it holds let go: the mask before its last step, if a
    /// modifier made it, given back to `bounds`.
    fn into_mask(self, bounds: Bounds) -> Arc<Mask> {
        let n = self.steps.len();
        if let Some(before) = self.before
            && n >= 2
            && self.steps[n - 2].modifies()
        {
            give_back(before, bounds);
        }
        self.mask
    }

    /// The selection `steps` make, its mask `mask`, which they came to (as a file kept it), not
    /// worked out again.
    pub fn kept(steps: Vec<Step>, mask: Arc<Mask>) -> Selection {
        Selection { steps, mask, before: None }
    }

    /// This selection with `step` after its steps (worked out from its mask, not again from
    /// the start; a step that starts over is the only one, and one that leaves it as it is,
    /// none).
    pub fn then(&self, step: Step) -> Selection {
        self.then_within(step, UNBOUNDED).expect("unbounded")
    }

    /// [`then`](Self::then), unless `bounds` hold a modifier's work back (one that leaves the
    /// selection as it is does none).
    pub fn then_within(&self, step: Step, bounds: Bounds) -> Result<Selection, Halt> {
        // No step at all.
        if step.leaves_as_is() {
            return Ok(self.clone());
        }
        if step.starts_over() {
            return Selection::of_within(vec![step], bounds);
        }
        let mask = step.apply(&self.mask, bounds)?;
        let mut steps = self.steps.clone();
        steps.push(step);
        Ok(Selection { steps, mask: Arc::new(mask), before: Some(self.mask.clone()) })
    }

    /// The selection `step` alone makes.
    pub fn new(step: Step) -> Selection {
        Selection::of(vec![step])
    }

    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    /// The mask its steps come to.
    pub fn mask(&self) -> &Arc<Mask> {
        &self.mask
    }

    /// Every mask it holds: the one its steps come to, the one they but the last come to (if
    /// kept), and those its mask steps keep.
    pub fn masks(&self) -> impl Iterator<Item = &Arc<Mask>> {
        std::iter::once(&self.mask).chain(&self.before).chain(self.steps.iter().filter_map(|s| match s {
            Step::Mask { mask, .. } => Some(mask),
            _ => None,
        }))
    }

    /// Whether it selects nothing at all.
    pub fn is_empty(&self) -> bool {
        self.mask.is_none()
    }

    /// Its last step, if that is a shape, and how it combines: what the box round it edits.
    pub fn last_shape(&self) -> Option<(Shape, Combine)> {
        match self.steps.last()? {
            Step::Shape { shape, combine } => Some((*shape, *combine)),
            _ => None,
        }
    }

    /// The selection its steps but the last make (nothing selected, if it has one step): its
    /// mask the one kept, if one was, else worked out again.
    pub fn before_last(&self) -> Selection {
        let steps = self.steps[..self.steps.len().saturating_sub(1)].to_vec();
        match &self.before {
            Some(mask) => Selection { steps, mask: mask.clone(), before: None },
            None => Selection::of(steps),
        }
    }

    /// The selection `m` carries this one to, as Rotate, Flip and Resize Image carry the
    /// layers: each step carried exactly, as far as they can be; where one cannot (a turned
    /// rectangle stretched along another way is no longer a rectangle), the selection up to
    /// it becomes its mask, carried pixel by pixel (see [`Mask::transformed`]).
    pub fn transformed(&self, m: &Affine) -> Selection {
        self.transformed_within(m, UNBOUNDED).expect("unbounded")
    }

    /// [`transformed`](Self::transformed), unless `bounds` hold back the work of a modifier
    /// worked out again.
    pub fn transformed_within(&self, m: &Affine, bounds: Bounds) -> Result<Selection, Halt> {
        // The last step that cannot be carried, found before any is (carrying a mask step,
        // pixel by pixel, would be wasted on one that the mask taken in its place holds).
        let stuck = self.steps.iter().rposition(|s| !s.carried_by(m));
        let (mut steps, rest) = match stuck {
            None => (Vec::new(), &self.steps[..]),
            Some(last) => {
                // Worked out again, the mask up to it goes once carried (given back, if a
                // modifier made it).
                let (up_to, made) = match (&self.before, self.steps.len() - last) {
                    (_, 1) => (self.mask.clone(), false),
                    (Some(before), 2) => (before.clone(), false),
                    _ => {
                        let made = Selection::of_within(self.steps[..=last].to_vec(), bounds)?;
                        (made.into_mask(bounds), self.steps[last].modifies())
                    }
                };
                let carried =
                    Step::Mask { mask: Arc::new(up_to.transformed(m)), combine: Combine::Replace, of: MaskOf::Carried };
                if made {
                    give_back(up_to, bounds);
                }
                (vec![carried], &self.steps[last + 1..])
            }
        };
        steps.extend(rest.iter().map(|s| s.transformed(m).expect("every step after the last stuck is carried")));
        Selection::of_within(steps, bounds)
    }
}

/// Gives back to `bounds` what `mask`, a modifier's, took as it was made (see `Bounds`), if
/// this was the last hold on it.
fn give_back(mask: Arc<Mask>, bounds: Bounds) {
    if let Some(mask) = Arc::into_inner(mask) {
        (bounds.give_back)(mask.bytes());
    }
}

/// The selection a maker `step` (a shape, or a mask) makes with the document's selection
/// `before` (or with none).
pub fn with_maker(before: Option<&Selection>, step: Step) -> Selection {
    match before {
        Some(s) => s.then(step),
        // With nothing selected, adding is making, and taking away or keeping only what is in
        // both leaves nothing.
        None => match step {
            Step::Shape { shape, combine: Combine::Replace | Combine::Add } => {
                Selection::new(Step::Shape { shape, combine: Combine::Replace })
            }
            Step::Mask { mask, combine: Combine::Replace | Combine::Add, of } => {
                Selection::new(Step::Mask { mask, combine: Combine::Replace, of })
            }
            step => Selection::of(vec![step]),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mask::{Coverage, FULL, ShapeKind};

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Shape {
        Shape {
            kind: ShapeKind::Rectangle,
            center: [(x0 + x1) / 2.0, (y0 + y1) / 2.0],
            size: [x1 - x0, y1 - y0],
            angle: 0.0,
            smooth: true,
        }
    }

    #[test]
    fn steps_build_a_selection_and_one_that_starts_over_leaves_the_rest_out() {
        let a = Selection::new(Step::Shape { shape: rect(0.0, 0.0, 100.0, 100.0), combine: Combine::Replace });
        let b = a.then(Step::Shape { shape: rect(200.0, 0.0, 300.0, 100.0), combine: Combine::Add });
        assert_eq!(b.steps().len(), 2);
        assert_eq!([b.mask().level_at(50, 50), b.mask().level_at(250, 50)], [FULL, FULL]);
        let c = b.then(Step::Invert);
        assert_eq!([c.mask().level_at(50, 50), c.mask().level_at(150, 50)], [0, FULL]);
        // Worked out again from its steps, the same.
        assert_eq!(Selection::of(c.steps().to_vec()), c);
        // Its last step a shape, the box round it edits it; the steps before it make the rest.
        assert_eq!(b.last_shape(), Some((rect(200.0, 0.0, 300.0, 100.0), Combine::Add)));
        assert_eq!(b.before_last(), a);
        // Kept on the way, not worked out again, whether made step by step or all at once.
        assert!(Arc::ptr_eq(b.before_last().mask(), a.mask()), "worked out again");
        let all_at_once = Selection::of(c.steps().to_vec());
        assert_eq!(all_at_once.before_last(), b);
        assert!(all_at_once.masks().any(|m| Arc::ptr_eq(m, all_at_once.before_last().mask())));
        assert_eq!(c.last_shape(), None, "an inverting step is no shape");
        assert!(a.before_last().is_empty());
        // A new shape replacing it is all there is.
        let d = c.then(Step::Shape { shape: rect(0.0, 0.0, 10.0, 10.0), combine: Combine::Replace });
        assert_eq!(d.steps().len(), 1);
        let all = Step::Shape { shape: rect(0.0, 0.0, 5.0, 5.0), combine: Combine::Replace };
        assert_eq!(Selection::of(vec![Step::Invert, all.clone()]).steps(), [all]);
    }

    #[test]
    fn a_recipe_worked_out_keeps_taken_only_what_its_last_two_masks_hold() {
        use crate::modify::{Bounds, UNBOUNDED};
        use std::sync::atomic::{AtomicI64, Ordering::SeqCst};
        let held = AtomicI64::new(0);
        let admit = |n: u64| {
            held.fetch_add(n as i64, SeqCst);
            true
        };
        let give_back = |n: u64| {
            held.fetch_sub(n as i64, SeqCst);
        };
        let bounds = Bounds { admit: &admit, give_back: &give_back, ..UNBOUNDED };
        let steps = vec![
            Step::Shape { shape: rect(100.0, 100.0, 900.0, 700.0), combine: Combine::Replace },
            Step::Grow { by: 12.0, corners: Reach::Round },
            Step::Grow { by: -5.0, corners: Reach::Round },
            Step::Grow { by: 7.0, corners: Reach::Round },
            Step::Grow { by: -3.0, corners: Reach::Round },
        ];
        let s = Selection::of_within(steps, bounds).unwrap();
        let kept = s.mask().bytes() + s.before.as_ref().map_or(0, |b| b.bytes());
        let held = held.into_inner();
        assert!(held > 0 && held as u64 <= kept, "{held} taken for {kept}");
    }

    #[test]
    fn a_modifier_worked_out_again_as_a_selection_is_carried_asks_for_its_memory() {
        use crate::modify::{Bounds, Halt, UNBOUNDED};
        let grown = Selection::new(Step::Shape { shape: rect(0.0, 0.0, 300.0, 100.0), combine: Combine::Replace })
            .then(Step::Grow { by: 20.0, corners: Reach::Round });
        // A quarter turn carries the grow, which is worked out again: within what is let.
        let quarter = Affine::rotate(std::f64::consts::FRAC_PI_2);
        let refuse = Bounds { admit: &|_| false, ..UNBOUNDED };
        assert_eq!(grown.transformed_within(&quarter, refuse).unwrap_err(), Halt::NoMemory);
        assert_eq!(grown.transformed_within(&quarter, UNBOUNDED).unwrap(), grown.transformed(&quarter));
    }

    #[test]
    fn a_selection_is_carried_through_a_transform_shape_by_shape_or_as_its_mask() {
        use crate::mask::{Extent, HALF};
        let a = Selection::new(Step::Shape { shape: rect(0.0, 0.0, 300.0, 100.0), combine: Combine::Replace }).then(
            Step::Shape {
                shape: Shape { kind: ShapeKind::Ellipse, ..rect(400.0, 0.0, 500.0, 50.0) },
                combine: Combine::Add,
            },
        );
        // A quarter turn clockwise about the origin: x goes to -y, y to x.
        let quarter = Affine::rotate(std::f64::consts::FRAC_PI_2);
        let b = a.transformed(&quarter);
        let Step::Shape { shape, .. } = b.steps()[0] else { panic!("{:?}", b.steps()[0]) };
        assert_eq!((shape.size, shape.angle), ([100.0, 300.0], 0.0), "on the grid again, its sides swapped");
        assert_eq!(b.mask().extent(), Extent::Within(crate::geom::IRect::new(-100, 0, 0, 500)));
        assert_eq!(b.mask().level_at(-50, 150), FULL);
        // Stretched along x, the rectangle and the ellipse stay shapes.
        let c = a.transformed(&Affine::scale(2.0, 1.0));
        assert!(c.steps().iter().all(|s| matches!(s, Step::Shape { .. })));
        assert_eq!(c.mask().extent(), Extent::Within(crate::geom::IRect::new(0, 0, 1000, 100)));
        // Turned by 30° and then stretched, the rectangle is no longer one: what was selected
        // up to it is carried as its mask; the ellipse after it is still a shape.
        let turned = Selection::new(Step::Shape {
            shape: Shape { angle: 30.0, ..rect(0.0, 0.0, 300.0, 100.0) },
            combine: Combine::Replace,
        })
        .then(Step::Shape {
            shape: Shape { kind: ShapeKind::Ellipse, ..rect(400.0, 0.0, 500.0, 50.0) },
            combine: Combine::Add,
        })
        .then(Step::Invert);
        let stretch = Affine::scale(2.0, 1.0);
        let d = turned.transformed(&stretch);
        assert!(matches!(d.steps(), [Step::Mask { .. }, Step::Shape { .. }, Step::Invert]), "{:?}", d.steps());
        // Where the stretched rectangle is (its middle, carried) unselected once inverted, and
        // its outline where the stretched rectangle's corners are.
        assert_eq!(d.mask().level_at(300, 50), 0);
        // Its corner 150 left and 50 up of its middle, along its sides (an obtuse corner once
        // stretched).
        let (s30, c30) = 30f64.to_radians().sin_cos();
        let corner = stretch.apply([150.0 - 150.0 * c30 + 50.0 * s30, 50.0 - 150.0 * s30 - 50.0 * c30]);
        let lines = d.mask().outline(HALF);
        let near =
            lines.iter().flatten().map(|p| (p[0] - corner[0]).hypot(p[1] - corner[1])).fold(f64::INFINITY, f64::min);
        assert!(near < 2.0, "the nearest point of the outline is {near} from the corner {corner:?}");
        // A turned ellipse stretched is another ellipse: every point of its edge, stretched,
        // is on the edge of the one it is carried to.
        let e = Shape { kind: ShapeKind::Ellipse, center: [7.0, -3.0], size: [200.0, 80.0], angle: 30.0, smooth: true };
        let carried = e.transformed(&stretch).expect("an ellipse stretched is one");
        for i in 0..32 {
            let t = f64::from(i) / 32.0 * std::f64::consts::TAU;
            let (x, y) = (100.0 * t.cos(), 40.0 * t.sin());
            let q = stretch.apply([7.0 + x * c30 - y * s30, -3.0 + x * s30 + y * c30]);
            let off = carried.distance(q[0], q[1]);
            assert!(off.abs() < 1e-6, "{q:?} is {off} from the edge of {carried:?}");
        }
    }

    #[test]
    fn a_mask_step_alone_is_the_selection_s_mask_and_is_not_read_back() {
        let m = Arc::new(Mask::of(&rect(0.0, 0.0, 300.0, 100.0)));
        let s = Selection::of(vec![Step::Mask { mask: m.clone(), combine: Combine::Replace, of: MaskOf::LayerPixels }]);
        assert!(Arc::ptr_eq(s.mask(), &m), "the mask was copied");
        let told = serde_json::to_string(&s.steps()[0]).unwrap();
        assert_eq!(told, r#"{"step":"mask","combine":"replace","of":"layer_pixels"}"#);
        assert!(serde_json::from_str::<Step>(&told).is_err(), "read back as no mask at all");
        // Combined, as a shape is: added to a rectangle beside it, both selected.
        let beside = Selection::new(Step::Shape { shape: rect(400.0, 0.0, 500.0, 100.0), combine: Combine::Replace });
        let both = beside.then(Step::Mask { mask: m, combine: Combine::Add, of: MaskOf::LayerPixels });
        assert_eq!(both.steps().len(), 2);
        assert_eq!([both.mask().level_at(50, 50), both.mask().level_at(450, 50)], [FULL, FULL]);
    }

    #[test]
    fn grown_and_feathered_steps_are_carried_by_even_scales_and_else_taken_as_their_mask() {
        let s = Selection::new(Step::Shape { shape: rect(100.0, 100.0, 200.0, 150.0), combine: Combine::Replace })
            .then(Step::Grow { by: 10.0, corners: Reach::Square })
            .then(Step::Feather { width: 4.0, side: Side::Inward, falloff: Falloff::Linear });
        assert_eq!(s.mask().extent(), crate::mask::Extent::Within(crate::geom::IRect::new(90, 90, 210, 160)));
        // Doubled every way: still steps, their distances doubled.
        let doubled = s.transformed(&Affine::scale(2.0, 2.0));
        assert!(matches!(doubled.steps()[1], Step::Grow { by: 20.0, .. }), "{:?}", doubled.steps());
        assert!(matches!(doubled.steps()[2], Step::Feather { width: 8.0, .. }));
        // Turned a quarter, so too, unscaled.
        let turned = s.transformed(&Affine::rotate(std::f64::consts::FRAC_PI_2));
        assert!(matches!(turned.steps()[1], Step::Grow { by, .. } if (by - 10.0).abs() < 1e-9));
        // Turned otherwise, square corners are not square to the grid: the selection up to the
        // grow is taken as its mask, the feather after it carried; round ones are carried.
        let thirty = Affine::rotate(30f64.to_radians());
        assert!(matches!(s.transformed(&thirty).steps(), [Step::Mask { .. }, Step::Feather { .. }]));
        let round = Selection::new(Step::Shape { shape: rect(100.0, 100.0, 200.0, 150.0), combine: Combine::Replace })
            .then(Step::Grow { by: 10.0, corners: Reach::Round });
        assert!(matches!(round.transformed(&thirty).steps(), [_, Step::Grow { .. }]));
        // Scaled past the farthest a step may reach, it is not carried (and so is taken as its
        // mask with the steps before it).
        let far = Affine::scale(200.0, 200.0);
        assert!(Step::Grow { by: 10.0, corners: Reach::Round }.transformed(&far).is_none());
        assert!(Step::Feather { width: 4.0, side: Side::Inward, falloff: Falloff::Linear }.transformed(&far).is_some());
        // A step that changes nothing is no step.
        assert_eq!(s.then(Step::Grow { by: 0.0, corners: Reach::Round }), s);
        assert_eq!(s.then(Step::Feather { width: 0.0, side: Side::Centered, falloff: Falloff::Linear }), s);
        // Stretched one way only: the selection up to the feather is taken as its mask.
        let stretched = s.transformed(&Affine::scale(2.0, 1.0));
        assert!(matches!(stretched.steps(), [Step::Mask { .. }]), "{:?}", stretched.steps());
    }

    #[test]
    fn a_step_that_leaves_the_selection_as_it_is_is_no_step_among_others() {
        let s = Selection::of(vec![
            Step::Shape { shape: rect(0.0, 0.0, 10.0, 10.0), combine: Combine::Replace },
            Step::Grow { by: 0.0, corners: Reach::Round },
            Step::Smooth { radius: 0.0 },
        ]);
        assert_eq!(s.steps().len(), 1, "{:?}", s.steps());
    }

    #[test]
    fn a_maker_with_nothing_selected_makes_it_when_added_or_leaves_nothing() {
        let s = rect(0.0, 0.0, 10.0, 10.0);
        let shape = |combine| with_maker(None, Step::Shape { shape: s, combine });
        assert_eq!(shape(Combine::Add).steps()[0], Step::Shape { shape: s, combine: Combine::Replace });
        assert!(shape(Combine::Subtract).is_empty());
        assert!(shape(Combine::Intersect).is_empty());
        let mask = Arc::new(Mask::of(&s));
        let made = with_maker(None, Step::Mask { mask: mask.clone(), combine: Combine::Add, of: MaskOf::LayerPixels });
        assert!(matches!(made.steps(), [Step::Mask { combine: Combine::Replace, .. }]));
        assert!(with_maker(None, Step::Mask { mask, combine: Combine::Intersect, of: MaskOf::LayerPixels }).is_empty());
    }
}
