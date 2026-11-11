//! A selection (design.md, *Selections*, *The recipe*): the steps that made it, in order, and
//! the mask they come to. A document holds one, or none (nothing selected: operations act on
//! whole layers), and so the history keeps it with the rest of each state.

use crate::geom::Affine;
use crate::mask::{Combine, Mask, Shape};
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
    /// A mask as it is, which replaces the selection so far: what a selection carried through
    /// a transform its shapes could not follow becomes (see [`Selection::transformed`]).
    Mask {
        #[serde(skip)]
        mask: Arc<Mask>,
    },
}

impl Step {
    /// The selection `before` (its mask) becomes after the step.
    fn apply(&self, before: &Mask) -> Mask {
        match self {
            Step::Shape { shape, combine } => {
                let made = Mask::of(shape);
                before.combined(&made, *combine)
            }
            Step::Invert => before.inverted(),
            Step::Mask { mask } => (**mask).clone(),
        }
    }

    /// Whether the step makes a selection of its own, the steps before it no longer counting.
    fn starts_over(&self) -> bool {
        matches!(self, Step::Shape { combine: Combine::Replace, .. } | Step::Mask { .. })
    }

    /// The step `m` carries this one to, if there is one (see [`Shape::transformed`]).
    fn transformed(&self, m: &Affine) -> Option<Step> {
        Some(match self {
            Step::Shape { shape, combine } => Step::Shape { shape: shape.transformed(m)?, combine: *combine },
            Step::Invert => Step::Invert,
            Step::Mask { mask } => Step::Mask { mask: Arc::new(mask.transformed(m)) },
        })
    }
}

/// A selection: see the module's description. Its mask is worked out from its steps when it
/// is made, and shared by its copies.
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    steps: Vec<Step>,
    mask: Arc<Mask>,
}

impl Selection {
    /// The selection `steps` make, from nothing selected (the steps before the last that
    /// starts over are left out).
    pub fn of(steps: Vec<Step>) -> Selection {
        let first = steps.iter().rposition(Step::starts_over).unwrap_or(0);
        let steps = steps[first..].to_vec();
        let mask = steps.iter().fold(Mask::none(), |m, s| s.apply(&m));
        Selection { steps, mask: Arc::new(mask) }
    }

    /// This selection with `step` after its steps (worked out from its mask, not again from
    /// the start; a step that starts over is the only one).
    pub fn then(&self, step: Step) -> Selection {
        if step.starts_over() {
            return Selection::of(vec![step]);
        }
        let mask = step.apply(&self.mask);
        let mut steps = self.steps.clone();
        steps.push(step);
        Selection { steps, mask: Arc::new(mask) }
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

    /// Whether it selects nothing at all.
    pub fn is_empty(&self) -> bool {
        self.mask.is_none()
    }

    /// The selection `m` carries this one to, as Rotate, Flip and Resize Image carry the
    /// layers: each step carried exactly, as far as they can be; where one cannot (a turned
    /// rectangle stretched along another way is no longer a rectangle), the selection up to
    /// it becomes its mask, carried pixel by pixel (see [`Mask::transformed`]).
    pub fn transformed(&self, m: &Affine) -> Selection {
        let carried: Vec<Option<Step>> = self.steps.iter().map(|s| s.transformed(m)).collect();
        let Some(last) = carried.iter().rposition(Option::is_none) else {
            return Selection::of(carried.into_iter().flatten().collect());
        };
        let up_to = if last + 1 == self.steps.len() {
            self.mask.clone()
        } else {
            Selection::of(self.steps[..=last].to_vec()).mask
        };
        let mut steps = vec![Step::Mask { mask: Arc::new(up_to.transformed(m)) }];
        steps.extend(carried.into_iter().skip(last + 1).flatten());
        Selection::of(steps)
    }
}

/// The selection a shape step makes with the document's selection `before` (or with none).
pub fn with_shape(before: Option<&Selection>, shape: Shape, combine: Combine) -> Selection {
    let step = Step::Shape { shape, combine };
    match before {
        Some(s) => s.then(step),
        // With nothing selected, adding is making, and taking away or keeping only what is in
        // both leaves nothing.
        None => match combine {
            Combine::Replace | Combine::Add => Selection::new(Step::Shape { shape, combine: Combine::Replace }),
            Combine::Subtract | Combine::Intersect => Selection::of(vec![step]),
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
        // A new shape replacing it is all there is.
        let d = c.then(Step::Shape { shape: rect(0.0, 0.0, 10.0, 10.0), combine: Combine::Replace });
        assert_eq!(d.steps().len(), 1);
        let all = Step::Shape { shape: rect(0.0, 0.0, 5.0, 5.0), combine: Combine::Replace };
        assert_eq!(Selection::of(vec![Step::Invert, all.clone()]).steps(), [all]);
    }

    #[test]
    fn a_selection_is_carried_through_a_transform_shape_by_shape_or_as_its_mask() {
        use crate::mask::{Extent, HALF};
        let a = Selection::new(Step::Shape { shape: rect(0.0, 0.0, 300.0, 100.0), combine: Combine::Replace })
            .then(Step::Shape { shape: Shape { kind: ShapeKind::Ellipse, ..rect(400.0, 0.0, 500.0, 50.0) }, combine: Combine::Add });
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
        let turned = Selection::new(Step::Shape { shape: Shape { angle: 30.0, ..rect(0.0, 0.0, 300.0, 100.0) }, combine: Combine::Replace })
            .then(Step::Shape { shape: Shape { kind: ShapeKind::Ellipse, ..rect(400.0, 0.0, 500.0, 50.0) }, combine: Combine::Add })
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
        let near = lines.iter().flatten().map(|p| (p[0] - corner[0]).hypot(p[1] - corner[1])).fold(f64::INFINITY, f64::min);
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
    fn a_shape_with_nothing_selected_is_made_added_or_leaves_nothing() {
        let s = rect(0.0, 0.0, 10.0, 10.0);
        assert_eq!(with_shape(None, s, Combine::Add).steps()[0], Step::Shape { shape: s, combine: Combine::Replace });
        assert!(with_shape(None, s, Combine::Subtract).is_empty());
        assert!(with_shape(None, s, Combine::Intersect).is_empty());
    }
}
