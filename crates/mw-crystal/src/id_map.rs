//! What an edit did to the sites and atoms that selections and measurements
//! refer to (design §7.11).

use std::collections::BTreeMap;

use crate::{AtomRef, SiteId};

/// How an edit changed one site, as far as references to it are concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SiteFate {
    Removed,
    /// The orbit changed size, so its member numbers may name other atoms
    /// than before; references keep only the site.
    Reshaped,
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

/// The sites an edit removed or reshaped. Sites it does not mention, and
/// references to their atoms, are unchanged.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IdMap {
    sites: BTreeMap<SiteId, SiteFate>,
}

impl IdMap {
    pub(crate) fn removed(site: SiteId) -> Self {
        Self {
            sites: [(site, SiteFate::Removed)].into(),
        }
    }

    pub(crate) fn reshaped(sites: impl IntoIterator<Item = SiteId>) -> Self {
        Self {
            sites: sites
                .into_iter()
                .map(|site| (site, SiteFate::Reshaped))
                .collect(),
        }
    }

    /// Whether every reference stays valid.
    pub fn is_identity(&self) -> bool {
        self.sites.is_empty()
    }

    pub fn site_fate(&self, site: SiteId) -> Option<SiteFate> {
        self.sites.get(&site).copied()
    }

    /// The site after the edit, or `None` if it was removed.
    pub fn site(&self, site: SiteId) -> Option<SiteId> {
        match self.site_fate(site) {
            Some(SiteFate::Removed) => None,
            Some(SiteFate::Reshaped) | None => Some(site),
        }
    }

    pub fn atom(&self, atom: AtomRef) -> AtomFate {
        match self.site_fate(atom.site) {
            None => AtomFate::Kept(atom),
            Some(SiteFate::Reshaped) => AtomFate::Site(atom.site),
            Some(SiteFate::Removed) => AtomFate::Removed,
        }
    }

    /// This map followed by `next`. A site removed by either stays removed,
    /// even if `next` restores it: references to it are dropped once.
    pub(crate) fn then(mut self, next: IdMap) -> Self {
        for (site, fate) in next.sites {
            let entry = self.sites.entry(site).or_insert(fate);
            if fate == SiteFate::Removed {
                *entry = SiteFate::Removed;
            }
        }
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
        assert_eq!(map.site(SiteId(1)), None);
        assert_eq!(map.site(SiteId(2)), Some(SiteId(2)));
        assert!(!map.is_identity());
        assert!(IdMap::default().is_identity());
    }

    #[test]
    fn removal_wins_when_composing() {
        let map = IdMap::reshaped([SiteId(1)]).then(IdMap::removed(SiteId(1)));
        assert_eq!(map.site_fate(SiteId(1)), Some(SiteFate::Removed));
        let map = IdMap::removed(SiteId(1)).then(IdMap::reshaped([SiteId(1)]));
        assert_eq!(map.site_fate(SiteId(1)), Some(SiteFate::Removed));
    }
}
