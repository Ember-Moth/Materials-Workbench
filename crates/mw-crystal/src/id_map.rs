//! What an edit did to the sites and atoms that selections and measurements
//! refer to (design §7.11).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::lattice::norm;
use crate::{AtomRef, SiteId, Structure, expand};

/// How an edit changed one site, as far as references to it are concerned.
#[derive(Clone, Debug, PartialEq)]
pub enum SiteFate {
    Removed,
    /// The orbit changed size, so its member numbers may name other atoms
    /// than before; references keep only the site.
    Reshaped,
    /// Expanded to P1 (`ExpandToP1`): member `k` became the only atom of
    /// site `k` of the list, in the same cell.
    Split(Vec<SiteId>),
    /// The site's only atom is now member `member` of `site`, in the same
    /// cell: a split undone.
    Merged {
        site: SiteId,
        member: u16,
    },
    /// The cell changed (`TransformCell`): the site's atoms are the same,
    /// with new member numbers and cells.
    Rebased(Box<Rebasing>),
    /// The structure was replaced by one with other sites (`Symmetrize`
    /// and its inverse): each of the site's atoms, at `before` (its orbit
    /// members in [0, 1)), is now the nearest atom of the new structure,
    /// and the new sites whose atoms are nearest to its atoms are `sites`.
    Relocated {
        before: Vec<[f64; 3]>,
        sites: Vec<SiteId>,
        to: Arc<Relocation>,
    },
}

/// An affine map of fractional coordinates, `x' = linear·x + shift`: where
/// a position in one cell lies in another (design §7.10, §7.11).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PositionMap {
    /// `linear[i][j]` is the entry in row i, column j.
    linear: [[f64; 3]; 3],
    shift: [f64; 3],
}

impl PositionMap {
    pub const IDENTITY: PositionMap = PositionMap {
        linear: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        shift: [0.0; 3],
    };

    pub(crate) fn new(linear: [[f64; 3]; 3], shift: [f64; 3]) -> Self {
        Self { linear, shift }
    }

    pub fn apply(&self, x: [f64; 3]) -> [f64; 3] {
        [0, 1, 2].map(|i| (0..3).map(|j| self.linear[i][j] * x[j]).sum::<f64>() + self.shift[i])
    }

    /// This map followed by `next`.
    pub(crate) fn then(&self, next: &PositionMap) -> PositionMap {
        let linear = [0, 1, 2].map(|i| {
            [0, 1, 2].map(|j| (0..3).map(|k| next.linear[i][k] * self.linear[k][j]).sum())
        });
        let moved = next.apply(self.shift);
        PositionMap {
            linear,
            shift: moved,
        }
    }

    /// The map back; `None` when the linear part is singular.
    pub fn inverse(&self) -> Option<PositionMap> {
        let inverse = nalgebra::Matrix3::from_fn(|i, j| self.linear[i][j]).try_inverse()?;
        let linear = [0, 1, 2].map(|i| [0, 1, 2].map(|j| inverse[(i, j)]));
        let shift = [0, 1, 2].map(|i| -(0..3).map(|j| linear[i][j] * self.shift[j]).sum::<f64>());
        Some(PositionMap { linear, shift })
    }
}

/// The atoms of a structure that replaced another, for finding where the
/// old atoms went (design §7.11, "`Symmetrize`: by nearest atom").
#[derive(Clone, Debug, PartialEq)]
pub struct Relocation {
    /// From the old structure's fractional coordinates to the new one's.
    map: PositionMap,
    /// The new cell's metric tensor, Å².
    metric: [[f64; 3]; 3],
    /// Every orbit member of the new structure, in [0, 1).
    atoms: Vec<(SiteId, u16, [f64; 3])>,
}

impl Relocation {
    /// The atom of the new structure nearest to where the old atom at `x`
    /// (old fractional coordinates, its cell included) lies.
    fn nearest(&self, x: [f64; 3]) -> Option<AtomRef> {
        nearest(&self.atoms, &self.metric, self.map.apply(x))
    }
}

/// Of `atoms` (orbit members in [0, 1)), the one nearest to `x` under the
/// metric, with the cell it is in.
fn nearest(
    atoms: &[(SiteId, u16, [f64; 3])],
    metric: &[[f64; 3]; 3],
    x: [f64; 3],
) -> Option<AtomRef> {
    let mut best: Option<(f64, AtomRef)> = None;
    for &(site, member, position) in atoms {
        let d = [0, 1, 2].map(|i| x[i] - position[i]);
        let cell = d.map(f64::round);
        let distance = norm(metric, [0, 1, 2].map(|i| d[i] - cell[i]));
        if best.is_none_or(|(nearest, _)| distance < nearest) {
            let atom = AtomRef::new(site, member, cell.map(|v| v as i32));
            best = Some((distance, atom));
        }
    }
    best.map(|(_, atom)| atom)
}

/// Every orbit member of a structure, in [0, 1).
fn members(structure: &Structure) -> Vec<(SiteId, u16, [f64; 3])> {
    let mut atoms = Vec::new();
    for site in &structure.sites {
        for (member, (_, position)) in expand::members(structure, site).into_iter().enumerate() {
            atoms.push((site.id, member as u16, position));
        }
    }
    atoms
}

/// How the atoms of one site are numbered after a change of cell: an atom
/// at fractional `x` in the old cell is at `linear·x + shift` in the new
/// one (design §7.8, §7.11).
#[derive(Clone, Debug, PartialEq)]
pub struct Rebasing {
    linear: [[f64; 3]; 3],
    shift: [f64; 3],
    /// The orbit members' positions in [0, 1), before and after.
    before: Vec<[f64; 3]>,
    after: Vec<[f64; 3]>,
}

impl Rebasing {
    pub(crate) fn new(
        linear: [[f64; 3]; 3],
        shift: [f64; 3],
        before: Vec<[f64; 3]>,
        after: Vec<[f64; 3]>,
    ) -> Self {
        Self {
            linear,
            shift,
            before,
            after,
        }
    }

    /// The same atom in the new cell, if it is found there.
    fn atom(&self, atom: AtomRef) -> Option<AtomRef> {
        // Members are at least the position tolerance apart, far more than
        // this in fractional coordinates.
        const SAME: f64 = 1e-6;
        let base = self.before.get(usize::from(atom.member))?;
        let x = [0, 1, 2].map(|i| base[i] + f64::from(atom.cell[i]));
        let moved = [0, 1, 2]
            .map(|i| (0..3).map(|j| self.linear[i][j] * x[j]).sum::<f64>() + self.shift[i]);
        self.after.iter().enumerate().find_map(|(member, p)| {
            let d = [0, 1, 2].map(|i| moved[i] - p[i]);
            d.iter()
                .all(|v| (v - v.round()).abs() < SAME)
                .then(|| AtomRef::new(atom.site, member as u16, d.map(|v| v.round() as i32)))
        })
    }
}

/// What became of an atom reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtomFate {
    /// The atom still exists under this reference.
    Kept(AtomRef),
    /// Only its site is known; a selection keeps the whole site.
    Site(SiteId),
    Removed,
}

/// The sites an edit changed, step by step for a batch. Sites it does not
/// mention, and references to their atoms, are unchanged.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IdMap {
    steps: Vec<BTreeMap<SiteId, SiteFate>>,
}

impl IdMap {
    fn single(sites: BTreeMap<SiteId, SiteFate>) -> Self {
        Self {
            steps: if sites.is_empty() {
                Vec::new()
            } else {
                vec![sites]
            },
        }
    }

    pub(crate) fn removed(site: SiteId) -> Self {
        Self::single([(site, SiteFate::Removed)].into())
    }

    pub(crate) fn reshaped(sites: impl IntoIterator<Item = SiteId>) -> Self {
        Self::single(
            sites
                .into_iter()
                .map(|site| (site, SiteFate::Reshaped))
                .collect(),
        )
    }

    pub(crate) fn with_fates(fates: impl IntoIterator<Item = (SiteId, SiteFate)>) -> Self {
        Self::single(fates.into_iter().collect())
    }

    /// `before` replaced by `after`, whose fractional coordinates `map`
    /// gives for `before`'s: every atom becomes the nearest atom of `after`.
    pub(crate) fn relocated(before: &Structure, after: &Structure, map: PositionMap) -> Self {
        let old_atoms = members(before);
        let to = Arc::new(Relocation {
            map,
            metric: after.lattice.params().metric(),
            atoms: members(after),
        });
        // The new sites that hold a site's atoms: every new atom belongs to
        // the old site of the old atom nearest to it, so a whole site of a
        // smaller cell takes all its atoms of the larger one.
        let mut sites: BTreeMap<SiteId, Vec<SiteId>> = BTreeMap::new();
        let mut add = |old: SiteId, new: SiteId| {
            let list = sites.entry(old).or_default();
            if !list.contains(&new) {
                list.push(new);
            }
        };
        let old_metric = before.lattice.params().metric();
        if let Some(back) = map.inverse() {
            for &(site, _, position) in &to.atoms {
                if let Some(old) = nearest(&old_atoms, &old_metric, back.apply(position)) {
                    add(old.site, site);
                }
            }
        }
        for &(site, _, position) in &old_atoms {
            if let Some(new) = to.nearest(position) {
                add(site, new.site);
            }
        }
        Self::with_fates(before.sites.iter().map(|site| {
            let before = old_atoms
                .iter()
                .filter(|(id, _, _)| *id == site.id)
                .map(|(_, _, position)| *position)
                .collect();
            let fate = SiteFate::Relocated {
                before,
                sites: sites.remove(&site.id).unwrap_or_default(),
                to: to.clone(),
            };
            (site.id, fate)
        }))
    }

    /// Whether every reference stays valid.
    pub fn is_identity(&self) -> bool {
        self.steps.is_empty()
    }

    /// The sites a site became: none if it was removed, several if it was
    /// split. A selection of the whole site selects them all.
    pub fn sites(&self, site: SiteId) -> Vec<SiteId> {
        let mut current = vec![site];
        for step in &self.steps {
            current = current
                .into_iter()
                .flat_map(|site| match step.get(&site) {
                    None | Some(SiteFate::Reshaped | SiteFate::Rebased(_)) => vec![site],
                    Some(SiteFate::Removed) => Vec::new(),
                    Some(SiteFate::Split(sites)) => sites.clone(),
                    Some(SiteFate::Merged { site, .. }) => vec![*site],
                    Some(SiteFate::Relocated { sites, .. }) => sites.clone(),
                })
                .collect();
            let mut seen = std::collections::BTreeSet::new();
            current.retain(|site| seen.insert(*site));
        }
        current
    }

    pub fn atom(&self, atom: AtomRef) -> AtomFate {
        let mut fate = AtomFate::Kept(atom);
        for step in &self.steps {
            fate = match fate {
                AtomFate::Removed => return AtomFate::Removed,
                AtomFate::Kept(atom) => match step.get(&atom.site) {
                    None => AtomFate::Kept(atom),
                    Some(SiteFate::Reshaped) => AtomFate::Site(atom.site),
                    Some(SiteFate::Removed) => AtomFate::Removed,
                    Some(SiteFate::Split(sites)) => match sites.get(usize::from(atom.member)) {
                        Some(site) => AtomFate::Kept(AtomRef::new(*site, 0, atom.cell)),
                        None => AtomFate::Removed,
                    },
                    Some(SiteFate::Merged { site, member }) if atom.member == 0 => {
                        AtomFate::Kept(AtomRef::new(*site, *member, atom.cell))
                    }
                    Some(SiteFate::Merged { site, .. }) => AtomFate::Site(*site),
                    Some(SiteFate::Rebased(rebasing)) => match rebasing.atom(atom) {
                        Some(atom) => AtomFate::Kept(atom),
                        None => AtomFate::Site(atom.site),
                    },
                    Some(SiteFate::Relocated { before, to, .. }) => {
                        let found = before.get(usize::from(atom.member)).and_then(|base| {
                            to.nearest([0, 1, 2].map(|i| base[i] + f64::from(atom.cell[i])))
                        });
                        found.map_or(AtomFate::Removed, AtomFate::Kept)
                    }
                },
                // A reference already reduced to its site follows the site;
                // of a split site it keeps the first part.
                AtomFate::Site(site) => match step.get(&site) {
                    None | Some(SiteFate::Reshaped | SiteFate::Rebased(_)) => AtomFate::Site(site),
                    Some(SiteFate::Removed) => AtomFate::Removed,
                    Some(SiteFate::Split(sites)) => sites
                        .first()
                        .map_or(AtomFate::Removed, |s| AtomFate::Site(*s)),
                    Some(SiteFate::Merged { site, .. }) => AtomFate::Site(*site),
                    Some(SiteFate::Relocated { before, to, .. }) => before
                        .first()
                        .and_then(|x| to.nearest(*x))
                        .map_or(AtomFate::Removed, |atom| AtomFate::Site(atom.site)),
                },
            };
        }
        fate
    }

    /// This map followed by `next`. A site removed by the first stays
    /// removed, even if `next` restores it: references to it are dropped
    /// once.
    pub(crate) fn then(mut self, next: IdMap) -> Self {
        self.steps.extend(next.steps);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atom(site: u32) -> AtomRef {
        AtomRef::new(SiteId(site), 2, [1, 0, 0])
    }

    #[test]
    fn fates_of_atoms() {
        let map = IdMap::removed(SiteId(1)).then(IdMap::reshaped([SiteId(2)]));
        assert_eq!(map.atom(atom(1)), AtomFate::Removed);
        assert_eq!(map.atom(atom(2)), AtomFate::Site(SiteId(2)));
        assert_eq!(map.atom(atom(3)), AtomFate::Kept(atom(3)));
        assert_eq!(map.sites(SiteId(1)), []);
        assert_eq!(map.sites(SiteId(2)), [SiteId(2)]);
        assert!(!map.is_identity());
        assert!(IdMap::default().is_identity());
        assert!(IdMap::reshaped([]).is_identity());
    }

    #[test]
    fn removal_wins_when_composing() {
        let map = IdMap::reshaped([SiteId(1)]).then(IdMap::removed(SiteId(1)));
        assert_eq!(map.atom(atom(1)), AtomFate::Removed);
        let map = IdMap::removed(SiteId(1)).then(IdMap::reshaped([SiteId(1)]));
        assert_eq!(map.atom(atom(1)), AtomFate::Removed);
        assert_eq!(map.sites(SiteId(1)), []);
    }

    #[test]
    fn split_sites_and_back() {
        let split = IdMap::with_fates([(
            SiteId(1),
            SiteFate::Split(vec![SiteId(5), SiteId(6), SiteId(7)]),
        )]);
        let member = AtomRef::new(SiteId(1), 2, [0, -1, 0]);
        let expanded = AtomRef::new(SiteId(7), 0, [0, -1, 0]);
        assert_eq!(split.atom(member), AtomFate::Kept(expanded));
        assert_eq!(split.sites(SiteId(1)), [SiteId(5), SiteId(6), SiteId(7)]);
        // Expanding, then removing one of the new sites.
        let then_removed = split.clone().then(IdMap::removed(SiteId(7)));
        assert_eq!(then_removed.atom(member), AtomFate::Removed);
        assert_eq!(then_removed.sites(SiteId(1)), [SiteId(5), SiteId(6)]);

        let merged = IdMap::with_fates((0..3).map(|k| {
            (
                SiteId(5 + k),
                SiteFate::Merged {
                    site: SiteId(1),
                    member: k as u16,
                },
            )
        }));
        assert_eq!(merged.atom(expanded), AtomFate::Kept(member));
        assert_eq!(merged.sites(SiteId(6)), [SiteId(1)]);
    }
}
