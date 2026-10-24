//! Displacement ellipsoids in the "ellipsoids" display model
//! (crystallography design §5.8, `docs/design/ui-shell.md` §5): the
//! probability they are drawn at, set in a popover beside the model's
//! toolbar button, which also names the sites drawn as small spheres for
//! want of parameters. Like the display model, the probability is view
//! state: not undone, not an unsaved change, and saved with the view.

use gpui_kit::component::input::{
    InputEvent, InputState, NumberInput, NumberInputEvent, StepAction,
};
use gpui_kit::component::{ActiveTheme, Sizable, StyledExt, h_flex, v_flex};
use gpui_kit::*;
use mw_crystal::{Displacements, SiteId, Structure};

use crate::actions::EditEllipsoids;
use crate::display_range::commits;
use crate::scene::DisplayModel;
use crate::workspace::StructureWorkspace;

/// Percent: the probabilities that can be set, whole percents short of
/// none and all.
const PERCENT_RANGE: std::ops::RangeInclusive<u32> = 1..=99;

/// Labels of the first sites, enough to recognize them in the popover.
const SITES_NAMED: usize = 4;

impl StructureWorkspace {
    /// The probability the displacement ellipsoids are drawn at.
    pub fn ellipsoid_probability(&self) -> f64 {
        self.ellipsoid_probability
    }

    /// Draws the ellipsoids at another probability, a fraction strictly
    /// between 0 and 1; others are ignored.
    pub fn set_ellipsoid_probability(&mut self, probability: f64, cx: &mut Context<Self>) {
        if probability > 0.0 && probability < 1.0 && self.ellipsoid_probability != probability {
            self.ellipsoid_probability = probability;
            self.refresh_scene(cx);
            cx.notify();
        }
    }

    /// Opens or closes the popover that sets the probability.
    pub fn set_ellipsoids_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.ellipsoids_open != open {
            self.ellipsoids_open = open;
            if open {
                let percent = percent(self.ellipsoid_probability);
                self.ellipsoids_panel
                    .update(cx, |panel, cx| panel.show(percent, window, cx));
            }
            cx.notify();
        }
    }

    /// The command palette's "Displacement ellipsoids…": shows the
    /// ellipsoids and opens the popover beside their button.
    pub(crate) fn edit_ellipsoids(
        &mut self,
        _: &EditEllipsoids,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dispatch_display_model(DisplayModel::Ellipsoids, window, cx);
        self.set_ellipsoids_open(true, window, cx);
    }

    /// The info chip's note in the ellipsoids model: how many sites are
    /// drawn as small spheres, if any.
    pub(crate) fn ellipsoid_info(&self) -> Option<SharedString> {
        if self.display_model != DisplayModel::Ellipsoids {
            return None;
        }
        let without = without_ellipsoids(self.document.structure());
        (without > 0).then(|| tr!("ellipsoids.info_without", count = without))
    }
}

/// The probability as a whole percent.
fn percent(probability: f64) -> u32 {
    (probability * 100.0).round() as u32
}

/// Sites drawn as small spheres: without displacement parameters, or with
/// parameters that make no ellipsoid.
fn without_ellipsoids(structure: &Structure) -> usize {
    let displacements = Displacements::new(structure, mw_crystal::DEFAULT_PROBABILITY);
    displacements.missing().len() + displacements.not_positive_definite().len()
}

/// "Ti1, O1", or "Ti1, O1, O2, O3 and 5 more".
fn site_list(structure: &Structure, sites: &[SiteId]) -> String {
    let labels: Vec<&str> = sites
        .iter()
        .take(SITES_NAMED)
        .filter_map(|id| structure.site(*id).map(|site| site.label()))
        .collect();
    let named = labels.join(&tr!("ellipsoids.separator"));
    if sites.len() > SITES_NAMED {
        tr!(
            "ellipsoids.and_more",
            sites = named,
            count = sites.len() - SITES_NAMED
        )
        .to_string()
    } else {
        named
    }
}

/// The popover's content: the probability as a whole percent, with a
/// stepper, and which sites are drawn as small spheres.
pub(crate) struct EllipsoidsPanel {
    workspace: WeakEntity<StructureWorkspace>,
    pub(crate) probability: Entity<InputState>,
    /// Why the value typed last cannot be used.
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl EllipsoidsPanel {
    pub(crate) fn new(
        workspace: WeakEntity<StructureWorkspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let probability = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(percent(mw_crystal::DEFAULT_PROBABILITY).to_string())
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &probability,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if commits(event) {
                        this.commit(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &probability,
                window,
                |this, _, event: &NumberInputEvent, window, cx| {
                    let NumberInputEvent::Step(step) = event;
                    this.step(*step, window, cx);
                },
            ),
        ];
        Self {
            workspace,
            probability,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    /// Shows the workspace's probability and forgets what was refused.
    fn show(&mut self, percent: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        self.probability.update(cx, |state, cx| {
            state.set_value(percent.to_string(), window, cx)
        });
        cx.notify();
    }

    fn apply(&mut self, percent: u32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        workspace.update(cx, |workspace, cx| {
            workspace.set_ellipsoid_probability(f64::from(percent) / 100.0, cx)
        });
        self.show(percent, window, cx);
    }

    fn typed(&self, cx: &App) -> Option<u32> {
        let text = self.probability.read(cx).value();
        let text = text.trim().trim_end_matches('%').trim_end();
        text.parse::<u32>()
            .ok()
            .filter(|percent| PERCENT_RANGE.contains(percent))
    }

    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.typed(cx) {
            Some(percent) => self.apply(percent, window, cx),
            None => {
                self.error = Some(tr!("ellipsoids.error"));
                cx.notify();
            }
        }
    }

    /// A stepper's button or arrow key: 10 percent more or fewer, from the
    /// value shown, within 1 to 99 %.
    fn step(&mut self, step: StepAction, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.typed(cx).unwrap_or_else(|| {
            self.workspace.upgrade().map_or(50, |workspace| {
                percent(workspace.read(cx).ellipsoid_probability)
            })
        });
        let next = match step {
            StepAction::Increment => (current / 10 + 1) * 10,
            StepAction::Decrement => current.saturating_sub(1) / 10 * 10,
        };
        let next = next.clamp(*PERCENT_RANGE.start(), *PERCENT_RANGE.end());
        self.apply(next, window, cx);
    }

    /// Why the value typed last cannot be used.
    #[cfg(test)]
    pub(crate) fn error(&self) -> Option<SharedString> {
        self.error.clone()
    }
}

impl Render for EllipsoidsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let label = |text: SharedString| div().text_xs().text_color(muted).child(text);
        let notes = self
            .workspace
            .upgrade()
            .map(|workspace| {
                let structure = workspace.read(cx).document.structure();
                let displacements = Displacements::new(structure, mw_crystal::DEFAULT_PROBABILITY);
                let mut notes = Vec::new();
                if !displacements.missing().is_empty() {
                    let sites = site_list(structure, displacements.missing());
                    notes.push(tr!("ellipsoids.missing", sites = sites));
                }
                if !displacements.not_positive_definite().is_empty() {
                    let sites = site_list(structure, displacements.not_positive_definite());
                    notes.push(tr!("ellipsoids.not_positive", sites = sites));
                }
                notes
            })
            .unwrap_or_default();

        v_flex()
            .id("ellipsoids-panel")
            .w_64()
            .gap_3()
            .text_sm()
            .child(div().font_medium().child(tr!("ellipsoids.title")))
            .child(
                v_flex()
                    .gap_1()
                    .child(label(tr!("ellipsoids.probability")))
                    .child(
                        h_flex().child(
                            div().w_32().child(
                                NumberInput::new(&self.probability)
                                    .small()
                                    .suffix(div().text_color(muted).child("%"))
                                    .accessibility_label(tr!("ellipsoids.probability")),
                            ),
                        ),
                    )
                    .child(label(tr!("ellipsoids.probability_hint"))),
            )
            .children(
                self.error
                    .clone()
                    .map(|error| div().text_xs().text_color(theme.danger).child(error)),
            )
            .children(notes.into_iter().map(label))
    }
}

#[cfg(test)]
mod tests {
    use super::percent;

    #[test]
    fn percents_round() {
        assert_eq!(percent(0.5), 50);
        assert_eq!(percent(0.995), 100);
        assert_eq!(percent(0.3), 30);
    }
}
