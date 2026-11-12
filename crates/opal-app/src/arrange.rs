//! Aligning and distributing the selected layers (design.md, *Layers UI*): Layer › Align,
//! the Move tool's settings, and scripts. Each layer goes by the bounds of what it shows (a
//! group by all inside it), and moves by whole pixels, as the Move tool moves it.

use crate::commands::Cmd;
use crate::doc::DocState;
use crate::state::AppState;
use opal_core::document::LayerId;
use opal_core::geom::{Affine, FRect};
use opal_core::mask::Extent;

/// Which edges, or centers, Align lines up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Left,
    HorizontalCenter,
    Right,
    Top,
    VerticalCenter,
    Bottom,
}

/// What Distribute spaces evenly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spread {
    HorizontalCenters,
    VerticalCenters,
    /// The gaps between them, side to side.
    HorizontalGaps,
    /// The gaps between them, top to bottom.
    VerticalGaps,
}

/// What an Align or Distribute command does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrange {
    Align(Edge),
    Distribute(Spread),
}

/// What the layers are aligned to, or distributed across (the Move tool's *Align to*).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum To {
    /// The box round the selected layers (aligning one, the canvas); distributing, among
    /// themselves.
    Layers,
    Canvas,
    /// The box round what is selected.
    Selection,
    /// The active layer, which stays where it is; distributing, as `Layers`.
    Key,
}

impl To {
    /// Their names, in the setting and in scripts' calls.
    pub const NAMES: [&str; 4] = ["layers", "canvas", "selection", "key"];

    /// The one called `name` (one of [`NAMES`](Self::NAMES)).
    pub fn named(name: &str) -> Option<To> {
        [To::Layers, To::Canvas, To::Selection, To::Key]
            .into_iter()
            .zip(Self::NAMES)
            .find(|(_, n)| *n == name)
            .map(|(t, _)| t)
    }

    /// The Move tool's *Align to* setting.
    pub fn chosen(st: &AppState) -> To {
        To::named(&st.tool_settings(crate::tools::ToolId::Move).choice("move.align_to")).unwrap_or(To::Layers)
    }
}

impl Arrange {
    /// What command `cmd` does, if it aligns or distributes.
    pub fn of(cmd: Cmd) -> Option<Arrange> {
        use Arrange::*;
        Some(match cmd {
            Cmd::AlignLeft => Align(Edge::Left),
            Cmd::AlignHorizontalCenters => Align(Edge::HorizontalCenter),
            Cmd::AlignRight => Align(Edge::Right),
            Cmd::AlignTop => Align(Edge::Top),
            Cmd::AlignVerticalCenters => Align(Edge::VerticalCenter),
            Cmd::AlignBottom => Align(Edge::Bottom),
            Cmd::DistributeHorizontalCenters => Distribute(Spread::HorizontalCenters),
            Cmd::DistributeVerticalCenters => Distribute(Spread::VerticalCenters),
            Cmd::DistributeHorizontalSpace => Distribute(Spread::HorizontalGaps),
            Cmd::DistributeVerticalSpace => Distribute(Spread::VerticalGaps),
            _ => return None,
        })
    }

    /// The history state it makes.
    fn label(self) -> &'static str {
        match self {
            Arrange::Align(Edge::Left) => "Align left edges",
            Arrange::Align(Edge::HorizontalCenter) => "Align horizontal centers",
            Arrange::Align(Edge::Right) => "Align right edges",
            Arrange::Align(Edge::Top) => "Align top edges",
            Arrange::Align(Edge::VerticalCenter) => "Align vertical centers",
            Arrange::Align(Edge::Bottom) => "Align bottom edges",
            Arrange::Distribute(Spread::HorizontalCenters) => "Distribute horizontal centers",
            Arrange::Distribute(Spread::VerticalCenters) => "Distribute vertical centers",
            Arrange::Distribute(Spread::HorizontalGaps) => "Distribute horizontal space",
            Arrange::Distribute(Spread::VerticalGaps) => "Distribute vertical space",
        }
    }

    /// Whether it moves things side to side (else up and down).
    fn horizontal(self) -> bool {
        matches!(
            self,
            Arrange::Align(Edge::Left | Edge::HorizontalCenter | Edge::Right)
                | Arrange::Distribute(Spread::HorizontalCenters | Spread::HorizontalGaps)
        )
    }
}

/// `r` along the axis `horizontal` names: its low and high sides.
fn span(r: &FRect, horizontal: bool) -> (f64, f64) {
    if horizontal { (r.x0, r.x1) } else { (r.y0, r.y1) }
}

fn center((lo, hi): (f64, f64)) -> f64 {
    (lo + hi) * 0.5
}

/// How far (whole pixels) to move each of `spans` (low and high sides along an axis) for its
/// point `at` of the way across (0 its low side, ½ its center, 1 its high side) to lie on
/// `target`'s.
fn aligned(spans: &[(f64, f64)], at: f64, target: (f64, f64)) -> Vec<i64> {
    let point = |(lo, hi): (f64, f64)| lo + (hi - lo) * at;
    spans.iter().map(|&s| (point(target) - point(s)).round() as i64).collect()
}

/// How far (whole pixels) to move each of `spans` (two or more) to space evenly their
/// centers, or (`gaps`) the gaps between them, taken in the order of their centers: between
/// the outermost two, which stay; or `across` a span, the first's low side on its low one and
/// the last's high side on its high one.
fn distributed(spans: &[(f64, f64)], gaps: bool, across: Option<(f64, f64)>) -> Vec<i64> {
    assert!(spans.len() >= 2, "distributing fewer than two");
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by(|&a, &b| center(spans[a]).total_cmp(&center(spans[b])));
    let (first, last) = (spans[order[0]], spans[order[order.len() - 1]]);
    let steps = (spans.len() - 1) as f64;
    let width = |(lo, hi): (f64, f64)| hi - lo;
    let mut out = vec![0; spans.len()];
    if gaps {
        let (start, end) = across.unwrap_or((first.0, last.1));
        let gap = (end - start - spans.iter().map(|&s| width(s)).sum::<f64>()) / steps;
        let mut at = start;
        for &i in &order {
            out[i] = (at - spans[i].0).round() as i64;
            at += width(spans[i]) + gap;
        }
    } else {
        let (from, to) = match across {
            Some((lo, hi)) => (lo + width(first) * 0.5, hi - width(last) * 0.5),
            None => (center(first), center(last)),
        };
        for (k, &i) in order.iter().enumerate() {
            let c = from + (to - from) * k as f64 / steps;
            out[i] = (c - center(spans[i])).round() as i64;
        }
    }
    out
}

/// The box round what is selected on `d`, or why there is none to align to.
fn selection_box(d: &DocState) -> Result<FRect, String> {
    let none = || "Nothing is selected to align to: select an area first.".to_owned();
    match d.pixel_selection().ok_or_else(none)?.mask().extent() {
        Extent::Within(r) => Ok(r.to_f()),
        Extent::Unbounded => {
            Err("The selection reaches past every edge: there is no box round it to align to.".to_owned())
        }
        Extent::Nothing => Err(none()),
    }
}

/// How `how` moves the selected layers of `d` to `to`: each that shows something, and how far
/// (whole pixels); or why it cannot.
fn plan(d: &DocState, how: Arrange, to: To) -> Result<Vec<(LayerId, [i64; 2])>, String> {
    let doc = d.doc();
    let canvas = doc.canvas.rect().to_f();
    let mut items: Vec<(LayerId, FRect)> =
        d.selected_layers().into_iter().filter_map(|l| Some((l, doc.layer(l)?.bounds()?))).collect();
    let horizontal = how.horizontal();
    let moves: Vec<i64> = match how {
        Arrange::Align(edge) => {
            let target = match to {
                To::Layers if items.len() >= 2 => items.iter().map(|i| i.1).reduce(|a, b| a.union(&b)).expect("two"),
                To::Layers | To::Canvas => canvas,
                To::Selection => selection_box(d)?,
                To::Key => {
                    let key = d.active_layer().filter(|&k| items.iter().any(|i| i.0 == k)).ok_or(
                        "The active layer, which the others line up to, must be one of those selected, and show something.",
                    )?;
                    let at = items.iter().position(|i| i.0 == key).expect("among them");
                    items.remove(at).1
                }
            };
            if items.is_empty() {
                return Err(match to {
                    To::Key => "Select other layers besides the active one to line them up to it.".to_owned(),
                    _ => "The selected layers show nothing to align.".to_owned(),
                });
            }
            let at = match edge {
                Edge::Left | Edge::Top => 0.0,
                Edge::HorizontalCenter | Edge::VerticalCenter => 0.5,
                Edge::Right | Edge::Bottom => 1.0,
            };
            let spans: Vec<_> = items.iter().map(|i| span(&i.1, horizontal)).collect();
            aligned(&spans, at, span(&target, horizontal))
        }
        Arrange::Distribute(spread) => {
            let across = match to {
                To::Layers | To::Key => None,
                To::Canvas => Some(span(&canvas, horizontal)),
                To::Selection => Some(span(&selection_box(d)?, horizontal)),
            };
            let least = if across.is_some() { 2 } else { 3 };
            if items.len() < least {
                return Err(if across.is_some() {
                    "Select two layers or more that show something to distribute them.".to_owned()
                } else {
                    "Select three layers or more that show something to distribute them between the outermost two \
                     (or distribute them across the canvas or the selection: Align to, in the Move tool's settings)."
                        .to_owned()
                });
            }
            let gaps = matches!(spread, Spread::HorizontalGaps | Spread::VerticalGaps);
            let spans: Vec<_> = items.iter().map(|i| span(&i.1, horizontal)).collect();
            distributed(&spans, gaps, across)
        }
    };
    let along = |m: i64| if horizontal { [m, 0] } else { [0, m] };
    Ok(items.iter().zip(moves).map(|(i, m)| (i.0, along(m))).collect())
}

/// Why `how` cannot act on the active image to `to` now, if it cannot: none open, an edit
/// under way, no layer selected, one to move locked, too few that show something, or nothing
/// selected to align to; with the transform box open, Distribute (Align moves the box).
pub fn refused(st: &AppState, how: Arrange, to: To) -> Result<(), String> {
    let d = st.active().ok_or("Open or create an image first.")?;
    if let Some(b) = &d.busy {
        return Err(b.why());
    }
    if d.transform.is_some() {
        return match (how, to) {
            (Arrange::Distribute(_), _) => Err("Apply or cancel the box first.".to_owned()),
            (Arrange::Align(_), To::Selection) => selection_box(d).map(|_| ()),
            (Arrange::Align(_), _) => Ok(()),
        };
    }
    if d.selected_layers().is_empty() {
        return Err("Select the layers to arrange in the Layers panel first.".to_owned());
    }
    if let Some(name) = crate::move_tool::locked_in_selected_layers(d) {
        return Err(format!("\"{name}\" is locked, so it cannot be moved. Unlock it first."));
    }
    plan(d, how, to).map(|_| ())
}

/// What the status bar says when nothing would move.
const ALREADY: &str = "Nothing moved: they are lined up so already.";

impl AppState {
    /// Aligns or distributes, as `how` says, the selected layers of the active image to `to`,
    /// as one history state; with the transform box open, aligns the box instead (to the
    /// selection, or else the canvas). Refused as [`refused`] says. When nothing would move,
    /// the status bar says so.
    pub(crate) fn arrange(&mut self, how: Arrange, to: To) -> Result<(), String> {
        refused(self, how, to)?;
        let d = self.active_mut().expect("refused without one");
        if let (Some(s), Arrange::Align(_)) = (&d.transform, how) {
            let target = if to == To::Selection { selection_box(d)? } else { d.doc().canvas.rect().to_f() };
            let corners = s.frame.corners().map(|p| s.current.apply(p));
            let around = FRect::bounding(&corners);
            let horizontal = how.horizontal();
            let at = match how {
                Arrange::Align(Edge::Left | Edge::Top) => 0.0,
                Arrange::Align(Edge::HorizontalCenter | Edge::VerticalCenter) => 0.5,
                _ => 1.0,
            };
            let m = aligned(&[span(&around, horizontal)], at, span(&target, horizontal))[0] as f64;
            let by = if horizontal { Affine::translate(m, 0.0) } else { Affine::translate(0.0, m) };
            if m == 0.0 {
                self.notify(ALREADY);
            } else {
                let moved = s.current.then(by);
                d.set_box(moved, None);
            }
            return Ok(());
        }
        let moves = plan(d, how, to)?;
        if moves.iter().all(|(_, m)| *m == [0, 0]) {
            self.notify(ALREADY);
            return Ok(());
        }
        let _ = d.apply(how.label(), |doc, _| {
            for &(id, m) in &moves {
                if m != [0, 0]
                    && let Some(l) = doc.layer_mut(id)
                {
                    crate::move_tool::shift(l, m);
                }
            }
            Ok::<(), ()>(())
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_line_up_by_an_edge_or_center_in_whole_pixels() {
        let spans = [(10.0, 30.0), (50.0, 60.0), (5.0, 105.0)];
        assert_eq!(aligned(&spans, 0.0, (0.0, 200.0)), vec![-10, -50, -5]);
        assert_eq!(aligned(&spans, 1.0, (0.0, 200.0)), vec![170, 140, 95]);
        // Centers: half a pixel rounds away from nothing.
        assert_eq!(aligned(&spans, 0.5, (0.0, 101.0)), vec![31, -5, -5]);
    }

    #[test]
    fn spans_spread_evenly_among_themselves_or_across_a_span() {
        // Centers at 10, 30 and 100 (in another order): the outer two stay, the middle one to 55.
        let spans = [(90.0, 110.0), (0.0, 20.0), (25.0, 35.0)];
        assert_eq!(distributed(&spans, false, None), vec![0, 0, 25]);
        // Gaps: 0..20, 25..35 and 90..110 leave 60 between them, 30 each: the middle one at 50..60.
        assert_eq!(distributed(&spans, true, None), vec![0, 0, 25]);
        // Across 0..200: the first's left side at 0, the last's right one at 200, the gaps even
        // ((200 - 50) / 2 = 75): 0..20, 95..105, 180..200.
        assert_eq!(distributed(&spans, true, Some((0.0, 200.0))), vec![90, 0, 70]);
        // Centers across it: from 10 (half the first's width in) to 190, the middle at 100.
        assert_eq!(distributed(&spans, false, Some((0.0, 200.0))), vec![90, 0, 70]);
        // Two across: one at each end.
        assert_eq!(distributed(&[(40.0, 50.0), (60.0, 80.0)], true, Some((0.0, 100.0))), vec![-40, 20]);
    }

    use crate::state::{Background, NewImage};
    use opal_core::document::{Document, IdGen, Layer, Position};
    use opal_core::geom::IRect;
    use opal_core::raster::RasterImage;

    /// An image 400 × 300 with a layer filled over each of `rects`, all selected, the first
    /// active; and their ids.
    fn image_with(rects: &[IRect]) -> (AppState, Vec<LayerId>) {
        let mut st = AppState::new();
        st.create_image(&NewImage { width: 400, height: 300, background: Background::Transparent, resolution: 72.0 });
        let red = opal_core::tile::pixel_from_f32([1.0, 0.0, 0.0, 1.0]);
        let mut ids = Vec::new();
        let d = st.active_mut().unwrap();
        d.apply("Layers", |doc: &mut Document, made: &mut IdGen| {
            for (i, r) in rects.iter().enumerate() {
                let id = made.layer();
                let layer = Layer::new_raster(id, made.object(), format!("L{i}"), RasterImage::filled(r, red));
                doc.insert(Position { parent: None, index: doc.layers.len() }, layer)?;
                ids.push(id);
            }
            Ok::<(), opal_core::document::TreeError>(())
        })
        .unwrap();
        d.select_only(ids[0]);
        for &id in &ids[1..] {
            d.toggle_selected(id);
        }
        d.make_active(ids[0]);
        (st, ids)
    }

    /// Where layer `id` of the active image shows: its bounds' left and top sides.
    fn at(st: &AppState, id: LayerId) -> [f64; 2] {
        let b = st.active().unwrap().doc().layer(id).unwrap().bounds().unwrap();
        [b.x0, b.y0]
    }

    #[test]
    fn layers_line_up_with_each_other_the_canvas_the_selection_or_the_active_one() {
        let rects = [IRect::new(10, 10, 50, 30), IRect::new(100, 60, 120, 100), IRect::new(200, 20, 260, 40)];
        let left = Arrange::Align(Edge::Left);
        // With each other: on the leftmost's left edge, as one history state.
        let (mut st, ids) = image_with(&rects);
        st.arrange(left, To::Layers).unwrap();
        assert_eq!(ids.iter().map(|&l| at(&st, l)[0]).collect::<Vec<_>>(), vec![10.0, 10.0, 10.0]);
        assert_eq!(st.active().unwrap().history.undo_label(), Some("Align left edges"));
        // Again: nothing moves, and the status bar says so.
        st.notice = None;
        st.arrange(left, To::Layers).unwrap();
        assert_eq!(st.notice.as_ref().map(|n| n.0.as_str()), Some(ALREADY));
        // Their bottoms on the canvas's; their vertical centers on the selection's.
        let (mut st, ids) = image_with(&rects);
        st.arrange(Arrange::Align(Edge::Bottom), To::Canvas).unwrap();
        assert!(ids.iter().all(|&l| st.active().unwrap().doc().layer(l).unwrap().bounds().unwrap().y1 == 300.0));
        let d = st.active_mut().unwrap();
        let shape = opal_core::mask::Shape::rectangle(&IRect::new(0, 100, 400, 200).to_f(), false);
        let step = opal_core::selection::Step::Shape { shape, combine: opal_core::mask::Combine::Replace };
        d.set_pixel_selection(Some(opal_core::selection::Selection::new(step)), "Select");
        st.arrange(Arrange::Align(Edge::VerticalCenter), To::Selection).unwrap();
        let centers: Vec<f64> = ids
            .iter()
            .map(|&l| {
                let b = st.active().unwrap().doc().layer(l).unwrap().bounds().unwrap();
                (b.y0 + b.y1) / 2.0
            })
            .collect();
        assert_eq!(centers, vec![150.0, 150.0, 150.0]);
        // To the active one (the middle), which stays: the others' right edges on its.
        let (mut st, ids) = image_with(&rects);
        st.active_mut().unwrap().make_active(ids[1]);
        st.arrange(Arrange::Align(Edge::Right), To::Key).unwrap();
        let rights: Vec<f64> =
            ids.iter().map(|&l| st.active().unwrap().doc().layer(l).unwrap().bounds().unwrap().x1).collect();
        assert_eq!(rights, vec![120.0, 120.0, 120.0]);
        assert_eq!(at(&st, ids[1]), [100.0, 60.0], "the active layer moved");
        // One layer alone: to the active one, nothing; to the canvas, else.
        let (mut st, ids) = image_with(&rects[1..2]);
        let why = refused(&st, Arrange::Align(Edge::Left), To::Key).unwrap_err();
        assert!(why.contains("besides the active one"), "{why}");
        st.arrange(Arrange::Align(Edge::HorizontalCenter), To::Layers).unwrap();
        assert_eq!(at(&st, ids[0])[0], 190.0);
    }

    #[test]
    fn layers_spread_among_themselves_or_across_the_canvas_and_say_why_not() {
        let rects = [IRect::new(0, 0, 20, 10), IRect::new(25, 0, 35, 10), IRect::new(90, 0, 110, 10)];
        let (mut st, ids) = image_with(&rects);
        st.arrange(Arrange::Distribute(Spread::HorizontalGaps), To::Layers).unwrap();
        assert_eq!(ids.iter().map(|&l| at(&st, l)[0]).collect::<Vec<_>>(), vec![0.0, 50.0, 90.0]);
        let (mut st, ids) = image_with(&rects);
        st.arrange(Arrange::Distribute(Spread::HorizontalCenters), To::Canvas).unwrap();
        // Centers from 10 to 390, the middle at 200.
        assert_eq!(ids.iter().map(|&l| at(&st, l)[0]).collect::<Vec<_>>(), vec![0.0, 195.0, 380.0]);
        // Two among themselves: too few; across the canvas, enough.
        let (st, _) = image_with(&rects[..2]);
        let spread = Arrange::Distribute(Spread::VerticalCenters);
        assert!(refused(&st, spread, To::Layers).unwrap_err().contains("three layers or more"));
        assert_eq!(refused(&st, spread, To::Canvas), Ok(()));
        // Nothing selected to align to; a layer locked; the box open, for Distribute.
        assert!(refused(&st, spread, To::Selection).unwrap_err().contains("Nothing is selected"));
        let (mut st, ids) = image_with(&rects);
        st.active_mut()
            .unwrap()
            .apply("Lock", |doc, _| {
                doc.layer_mut(ids[2]).unwrap().locked = true;
                Ok::<(), ()>(())
            })
            .unwrap();
        assert!(refused(&st, spread, To::Canvas).unwrap_err().contains("\"L2\" is locked"));
    }

    #[test]
    fn with_the_box_open_align_moves_the_box() {
        let (mut st, _) = image_with(&[IRect::new(10, 10, 50, 30)]);
        let d = st.active_mut().unwrap();
        assert!(d.begin_transform(opal_core::resample::Sampling::default()));
        st.arrange(Arrange::Align(Edge::Right), To::Layers).unwrap();
        let s = st.active().unwrap().transform.as_ref().unwrap();
        let right = s.frame.corners().map(|p| s.current.apply(p)).iter().map(|p| p[0]).fold(f64::MIN, f64::max);
        assert_eq!(right, 400.0);
        assert!(refused(&st, Arrange::Distribute(Spread::HorizontalGaps), To::Canvas).unwrap_err().contains("box"));
    }

    #[test]
    fn names_of_what_they_align_to_go_both_ways() {
        for name in To::NAMES {
            assert_eq!(To::NAMES[To::named(name).map(|t| t as usize).unwrap()], name);
        }
        assert_eq!(To::named("nothing"), None);
    }
}
