//! Measuring (`docs/design/ui-shell.md` §4.2): with Measure on, the first two
//! atoms clicked give their distance, and a third gives the angle at the
//! second. Clicking empty space starts over. Results go to the measurements
//! tab and appear as labels in the viewport.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mw_crystal::{Annotation, AtomRef, Command, Measurement};

use crate::document::StructureDocument;
use crate::format;
use crate::workspace::StructureWorkspace;

/// The atoms' site labels joined by dashes, e.g. `O1–Ti1–O1`.
pub(crate) fn label(document: &StructureDocument, measurement: &Measurement) -> String {
    measurement
        .atoms()
        .iter()
        .map(|atom| {
            document
                .structure()
                .site(atom.site)
                .map_or("?", |s| s.label())
        })
        .collect::<Vec<_>>()
        .join("–")
}

/// The value with its unit, or `None` when the measurement is stale.
pub(crate) fn annotation_value(
    document: &StructureDocument,
    annotation: &Annotation,
) -> Option<String> {
    if annotation.stale {
        return None;
    }
    value(document, &annotation.measurement)
}

/// The value with its unit, or `None` when an atom is missing.
pub(crate) fn value(document: &StructureDocument, measurement: &Measurement) -> Option<String> {
    let value = measurement.value(&document.structure().lattice, document.derived())?;
    Some(match measurement {
        Measurement::Distance(_) => format::distance(value),
        Measurement::Angle(_) => format::bond_angle(value),
    })
}

/// Offset from a distance's midpoint to its label's top-left corner, so the
/// label sits just above the bond. Measured runtime geometry.
const DISTANCE_LABEL_OFFSET: Point<Pixels> = Point {
    x: px(-32.),
    y: px(-30.),
};
/// Logical pixels from an angle's vertex into the angle, along the bisector
/// of its arms on screen, where the label is centered.
const ANGLE_LABEL_DISTANCE: f32 = 40.;
/// Offset that centers an angle label on its point. Measured runtime
/// geometry.
const ANGLE_LABEL_OFFSET: Point<Pixels> = Point {
    x: px(-24.),
    y: px(-11.),
};

/// The unit vector from `from` to `to` on screen, if they are apart.
fn direction(from: Point<Pixels>, to: Point<Pixels>) -> Option<(f32, f32)> {
    let (dx, dy) = (f32::from(to.x - from.x), f32::from(to.y - from.y));
    let length = dx.hypot(dy);
    (length > 1e-3).then(|| (dx / length, dy / length))
}

impl StructureWorkspace {
    /// Adds a clicked atom to the measurement in progress; `None` (empty
    /// space) starts over. A finished measurement is an undo step.
    pub(crate) fn measure_pick(
        &mut self,
        atom: Option<AtomRef>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match atom {
            None => self.measure_picks.clear(),
            Some(atom) if self.measure_picks.contains(&atom) => return,
            Some(atom) => {
                self.measure_picks.push(atom);
                let measurement = match self.measure_picks[..] {
                    [a, b] => Some(Measurement::Distance([a, b])),
                    [a, vertex, c] => {
                        self.measure_picks.clear();
                        Some(Measurement::Angle([a, vertex, c]))
                    }
                    _ => None,
                };
                if let Some(measurement) = measurement {
                    let _ = self.execute(Command::AddMeasurement(measurement), window, cx);
                }
            }
        }
        self.sync_viewport_selection(cx);
        cx.notify();
    }

    pub(crate) fn remove_measurement(
        &mut self,
        measurement: Measurement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let _ = self.execute(Command::RemoveMeasurement(measurement), window, cx);
    }

    /// The prompt shown over the viewport while measuring.
    pub(crate) fn measure_prompt(&self) -> SharedString {
        match self.measure_picks.len() {
            0 => tr!("measure.prompt.first"),
            1 => tr!("measure.prompt.second"),
            _ => tr!("measure.prompt.third"),
        }
    }

    /// Each measurement's value and the top-left corner of its label,
    /// relative to the viewport: above the middle of a distance, inside an
    /// angle.
    pub(crate) fn measurement_labels(&self, cx: &App) -> Vec<(Point<Pixels>, String)> {
        let viewport = self.viewport.read(cx);
        let document = &self.document;
        let project = |atom: AtomRef| {
            viewport.project(
                document
                    .derived()
                    .cartesian(&document.structure().lattice, atom)?,
            )
        };
        let position = |m: &Measurement| match *m {
            Measurement::Distance([a, b]) => {
                let (a, b) = (project(a)?, project(b)?);
                Some(point((a.x + b.x) / 2., (a.y + b.y) / 2.) + DISTANCE_LABEL_OFFSET)
            }
            Measurement::Angle([a, vertex, c]) => {
                let (a, vertex, c) = (project(a)?, project(vertex)?, project(c)?);
                let to_a = direction(vertex, a).unwrap_or((0., -1.));
                let to_c = direction(vertex, c).unwrap_or((0., -1.));
                let sum = point(px(to_a.0 + to_c.0), px(to_a.1 + to_c.1));
                // A straight angle has no inside; put its label above.
                let (bx, by) = direction(Point::default(), sum).unwrap_or((0., -1.));
                let inside = point(px(bx * ANGLE_LABEL_DISTANCE), px(by * ANGLE_LABEL_DISTANCE));
                Some(vertex + inside + ANGLE_LABEL_OFFSET)
            }
        };
        document
            .crystal
            .annotations()
            .iter()
            .filter_map(|a| Some((position(&a.measurement)?, annotation_value(document, a)?)))
            .collect()
    }

    pub(crate) fn render_measurements(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let rows =
            self.document
                .crystal
                .annotations()
                .iter()
                .enumerate()
                .map(|(ix, annotation)| {
                    let m = &annotation.measurement;
                    let measurement = *m;
                    let value = annotation_value(&self.document, annotation);
                    h_flex()
                        .id(("measurement", ix))
                        .gap_2()
                        .pl_2()
                        .py_0p5()
                        .text_sm()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(label(&self.document, m)),
                        )
                        .child(match value {
                            Some(value) => div()
                                .font_family(theme.mono_font_family.clone())
                                .child(value),
                            None => div()
                                .text_color(theme.muted_foreground)
                                .child(tr!("panel.measurements.stale")),
                        })
                        .child(
                            Button::new(("remove-measurement", ix))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .tooltip(tr!("panel.measurements.remove"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.remove_measurement(measurement, window, cx)
                                })),
                        )
                });
        v_flex()
            .p_2()
            .gap_1()
            .children(rows)
            .when(self.document.crystal.annotations().is_empty(), |this| {
                this.child(
                    div()
                        .p_2()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(tr!("panel.measurements.empty")),
                )
            })
            .child(
                div()
                    .px_2()
                    .pt_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(tr!("panel.measurements.hint")),
            )
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's glob would shadow the `test` attribute.
    use mw_crystal::sample::SampleKey;
    use mw_crystal::{AtomRef, Measurement, SiteId};

    use super::{label, value};
    use crate::document::StructureDocument;

    fn rutile() -> StructureDocument {
        StructureDocument::from_sample("p", "rutile", SampleKey::Rutile)
    }

    #[test]
    fn rutile_apical_bond_and_angle() {
        let document = rutile();
        // Member of a site's orbit at a fractional position, in the home cell.
        let at = |site: u32, p: [f64; 3]| {
            let orbit = document.derived().orbit(SiteId(site)).unwrap();
            let member = orbit
                .positions
                .iter()
                .position(|q| (0..3).all(|i| (q[i] - p[i]).abs() < 1e-9))
                .unwrap();
            AtomRef::new(SiteId(site), member as u16, [0; 3])
        };
        // The body-center Ti and its two apical O.
        let center = at(1, [0.5, 0.5, 0.5]);
        let apical = at(2, [0.8048, 0.1952, 0.5]);
        let other = at(2, [0.1952, 0.8048, 0.5]);
        let distance = Measurement::Distance([center, apical]);
        assert_eq!(label(&document, &distance), "Ti1–O1");
        assert_eq!(value(&document, &distance).as_deref(), Some("1.980 Å"));

        let angle = Measurement::Angle([apical, center, other]);
        assert_eq!(label(&document, &angle), "O1–Ti1–O1");
        assert_eq!(value(&document, &angle).as_deref(), Some("180.0°"));
    }

    #[test]
    fn stale_measurements_have_no_value() {
        let document = rutile();
        let gone = AtomRef::new(SiteId(9), 0, [0; 3]);
        let m = Measurement::Distance([gone, gone]);
        assert_eq!(label(&document, &m), "?–?");
        assert_eq!(value(&document, &m), None);
    }
}
