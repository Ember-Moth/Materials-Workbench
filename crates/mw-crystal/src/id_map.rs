//! What an edit did to the sites and atoms that selections and measurements
//! refer to (design §7.11).

use std::collections::BTreeMap;

use crate::{AtomRef, SiteId};

/// How an edit changed one site, as far as references to it are concerned.
#[derive(Clone, Debug, PartialEq, Eq)]
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
                    None | Some(SiteFate::Reshaped) => vec![site],
                    Some(SiteFate::Removed) => Vec::new(),
                    Some(SiteFate::Split(sites)) => sites.clone(),
                    Some(SiteFate::Merged { site, .. }) => vec![*site],
                })
                .collect();
            current.dedup();
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
                },
                // A reference already reduced to its site follows the site;
                // of a split site it keeps the first part.
                AtomFate::Site(site) => match step.get(&site) {
                    None | Some(SiteFate::Reshaped) => AtomFate::Site(site),
                    Some(SiteFate::Removed) => AtomFate::Removed,
                    Some(SiteFate::Split(sites)) => sites
                        .first()
                        .map_or(AtomFate::Removed, |s| AtomFate::Site(*s)),
                    Some(SiteFate::Merged { site, .. }) => AtomFate::Site(*site),
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
