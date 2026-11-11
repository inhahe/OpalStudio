//! A selection (design.md, *Selections*, *The recipe*): the steps that made it, in order, and
//! the mask they come to. A document holds one, or none (nothing selected: operations act on
//! whole layers), and so the history keeps it with the rest of each state.

use crate::mask::{Combine, Mask, Shape};
use std::sync::Arc;

/// A step of a selection's recipe: a maker, which makes a mask and combines it with the
/// selection so far, or a modifier, which changes the selection so far.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "step")]
pub enum Step {
    /// A rectangle or an ellipse, combined with the selection so far.
    Shape { shape: Shape, combine: Combine },
    /// Everything (Select All).
    All,
    /// The opposite of the selection so far: selected where it was not.
    Invert,
}

impl Step {
    /// The selection `before` (its mask) becomes after the step.
    fn apply(&self, before: &Mask) -> Mask {
        match self {
            Step::Shape { shape, combine } => {
                let made = Mask::of(shape);
                before.combined(&made, *combine)
            }
            Step::All => Mask::all(),
            Step::Invert => before.inverted(),
        }
    }

    /// Whether the step makes a selection of its own, the steps before it no longer counting.
    fn starts_over(&self) -> bool {
        matches!(self, Step::All | Step::Shape { combine: Combine::Replace, .. })
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
    use crate::mask::{FULL, ShapeKind};

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
        assert_eq!(Selection::of(vec![Step::Invert, Step::All]).steps(), [Step::All]);
    }

    #[test]
    fn a_shape_with_nothing_selected_is_made_added_or_leaves_nothing() {
        let s = rect(0.0, 0.0, 10.0, 10.0);
        assert_eq!(with_shape(None, s, Combine::Add).steps()[0], Step::Shape { shape: s, combine: Combine::Replace });
        assert!(with_shape(None, s, Combine::Subtract).is_empty());
        assert!(with_shape(None, s, Combine::Intersect).is_empty());
    }
}
