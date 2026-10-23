//! Where the geometric edits put a site (design §7.4–7.6): a dragged
//! atom's displacement taken back to its site and projected onto the site's
//! special position, a typed coordinate with the coordinates that follow
//! it, and a position pulled onto the special position it lies near.

use crate::site::Stabilizer;
use crate::{Axis, CoordinateConstraint, Structure, SymOp, special_position};

/// A position for a site, and how far snapping moved it there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Placement {
    pub position: [f64; 3],
    /// Å; 0 when the position was not snapped.
    pub snapped: f64,
}

impl Placement {
    pub fn at(position: [f64; 3]) -> Self {
        Self {
            position,
            snapped: 0.0,
        }
    }
}

fn stabilizer(structure: &Structure, x: [f64; 3]) -> Stabilizer {
    let metric = structure.lattice.params().metric();
    Stabilizer::new(
        &structure.symmetry,
        &metric,
        x,
        structure.tolerances.position,
    )
}

/// `position`, pulled onto the special position within the position
/// tolerance if that one has more than `order` operations: an atom that
/// comes that close to a special position belongs on it (design §7.4).
/// Otherwise `position` stays as it is, so that typed values stay exact.
pub(crate) fn settle(structure: &Structure, position: [f64; 3], order: usize) -> Placement {
    let special = special_position(structure, position);
    if special.order > order {
        Placement {
            position: special.position,
            snapped: special.displacement,
        }
    } else {
        Placement::at(position)
    }
}

/// Where a site at `x` goes when the atom that `op` makes of it moves by
/// the fractional `displacement` (design §7.4): the displacement is taken
/// back to the site, `W⁻¹·Δx`, and projected onto the directions the site's
/// special position allows, `P_s·W⁻¹·Δx`, unless `leave` asks to leave the
/// special position. A site that ends up near a special position with more
/// symmetry than it had snaps onto it.
pub(crate) fn moved(
    structure: &Structure,
    x: [f64; 3],
    op: &SymOp,
    displacement: [f64; 3],
    leave: bool,
) -> Placement {
    let inverse = inverse_rotation(op);
    let mut d = mul(&inverse.map(|row| row.map(f64::from)), displacement);
    let mut order = 1;
    if !leave {
        let current = stabilizer(structure, x);
        d = mul(&current.projector(), d);
        order = current.order();
    }
    settle(structure, [0, 1, 2].map(|i| x[i] + d[i]), order)
}

/// Where a site at `x` goes when its `axis` coordinate is set to `value`
/// on its special position (design §7.5), see
/// [`crate::CoordinateForm::with_coordinate`]; snapped when the new
/// position lies near a special position with more symmetry. Fails with
/// the coordinate's constraint when it is fixed or a combination.
pub(crate) fn with_coordinate(
    structure: &Structure,
    x: [f64; 3],
    axis: Axis,
    value: f64,
) -> Result<Placement, CoordinateConstraint> {
    let current = stabilizer(structure, x);
    let position = current.form().with_coordinate(x, axis, value)?;
    Ok(settle(structure, position, current.order()))
}

fn mul(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| (0..3).map(|j| m[i][j] * v[j]).sum())
}

/// `W⁻¹`, an integer matrix because the determinant is ±1: the adjugate
/// divided by the determinant.
fn inverse_rotation(op: &SymOp) -> [[i32; 3]; 3] {
    let w = op.rotation();
    let det = op.determinant();
    [0, 1, 2].map(|i| {
        [0, 1, 2].map(|j| {
            let (j1, j2, i1, i2) = ((j + 1) % 3, (j + 2) % 3, (i + 1) % 3, (i + 2) % 3);
            (w[j1][i1] * w[j2][i2] - w[j1][i2] * w[j2][i1]) * det
        })
    })
}

/// The geometric edits (design §7.4–7.6), with what they do to references
/// (§7.11) and undo (§7.12).
#[cfg(test)]
mod tests {
    use nalgebra::{Matrix3, Vector3};

    use crate::sample::{self, SampleKey};
    use crate::{
        Applied, AtomFate, AtomRef, CellParams, CrystalDocument, Derived, Edit, EditError,
        EditInfo, EditWarning, Guidance, Lattice, Measured, Site, SiteId, Species, Symmetry,
    };

    use super::*;

    fn rutile() -> Structure {
        sample::load(SampleKey::Rutile).structure
    }

    const O1: SiteId = SiteId(2);
    const TI1: SiteId = SiteId(1);
    const X: f64 = 0.3048;

    fn structure(generators: &[&str], params: CellParams, sites: Vec<Site>) -> Structure {
        let generators: Vec<SymOp> = generators
            .iter()
            .map(|g| SymOp::parse(g).unwrap())
            .collect();
        Structure::new(
            Lattice::new(params),
            Symmetry::generate(&generators).unwrap(),
            sites,
        )
    }

    /// Rutile's operations, P4₂/mnm, with one site.
    fn rutile_with(site: Site) -> Structure {
        let rutile = rutile();
        Structure::new(rutile.lattice, rutile.symmetry, vec![site])
    }

    /// P6₃/mmc with one site.
    fn hexagonal(x: [f64; 3]) -> Structure {
        structure(
            &["x-y,x,z+1/2", "y,x,-z", "-x,-y,-z"],
            CellParams::new(3.0, 3.0, 5.0, 90.0, 90.0, 120.0),
            vec![Site::single(1, "C1", "C", x)],
        )
    }

    /// Applies the edit, checks that its inverse restores the structure
    /// exactly and that the inverse of that redoes it.
    fn round_trip(structure: &Structure, edit: Edit) -> Applied {
        let applied = edit.apply(structure).expect("edit applies");
        let restored = applied
            .inverse
            .apply(&applied.structure)
            .expect("inverse applies");
        assert_eq!(&restored.structure, structure, "{edit:?}");
        let redone = restored.inverse.apply(structure).unwrap();
        assert_eq!(redone.structure, applied.structure, "redo of {edit:?}");
        applied
    }

    fn position(structure: &Structure, site: SiteId) -> [f64; 3] {
        structure.site(site).unwrap().position
    }

    fn assert_close(a: [f64; 3], b: [f64; 3]) {
        for i in 0..3 {
            assert!((a[i] - b[i]).abs() < 1e-12, "{a:?} != {b:?}");
        }
    }

    fn drag(atom: AtomRef, displacement: [f64; 3]) -> Edit {
        Edit::MoveSite {
            atom,
            displacement,
            leave_special_position: false,
        }
    }

    fn release(atom: AtomRef, displacement: [f64; 3]) -> Edit {
        Edit::MoveSite {
            atom,
            displacement,
            leave_special_position: true,
        }
    }

    fn atom(site: SiteId, member: u16) -> AtomRef {
        AtomRef::new(site, member, [0; 3])
    }

    /// Fractional coordinates of a Cartesian vector, through the lattice
    /// whatever its orientation.
    fn to_fractional(structure: &Structure, r: [f64; 3]) -> [f64; 3] {
        let basis = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
            .map(|e| structure.lattice.to_cartesian(e));
        let a = Matrix3::from_fn(|i, j| basis[j][i]);
        let x = a.try_inverse().unwrap() * Vector3::from(r);
        [x[0], x[1], x[2]]
    }

    #[test]
    fn dragging_keeps_a_site_on_its_special_position() {
        // O1 on 4f, x, x, 0: only the [110] component of the drag counts.
        let rutile = rutile();
        let applied = round_trip(&rutile, drag(atom(O1, 0), [0.01, 0.0, 0.02]));
        assert_close(
            position(&applied.structure, O1),
            [X + 0.005, X + 0.005, 0.0],
        );
        assert!(applied.info.is_empty() && applied.warnings.is_empty());
        // Same orbit, same members: references stay (design §7.11).
        assert!(applied.ids.is_identity());
        let derived = Derived::compute(&applied.structure);
        assert_eq!(derived.multiplicity(O1), Some(4));
    }

    #[test]
    fn a_dragged_atom_follows_the_gesture_whichever_it_is() {
        // Member (-x+1/2, x+1/2, 1/2) of 4f lies on a line along [-110]:
        // the displacement goes back to the site through W⁻¹ (design §7.4).
        let rutile = rutile();
        let derived = Derived::compute(&rutile);
        let member = derived
            .orbit(O1)
            .unwrap()
            .positions
            .iter()
            .position(|p| (p[0] - (0.5 - X)).abs() < 1e-12 && (p[1] - (0.5 + X)).abs() < 1e-12)
            .unwrap() as u16;
        let before = derived.fractional(atom(O1, member)).unwrap();
        let along = [-0.01, 0.01, 0.0];
        let applied = round_trip(&rutile, drag(atom(O1, member), along));
        let moved = Derived::compute(&applied.structure);
        let after = moved.fractional(atom(O1, member)).unwrap();
        assert_close(after, [0, 1, 2].map(|i| before[i] + along[i]));
        // Every equivalent atom moved with it: the site is at x + 0.01.
        assert_close(position(&applied.structure, O1), [X + 0.01, X + 0.01, 0.0]);

        // Across the line, the drag does nothing.
        let across = drag(atom(O1, member), [0.01, 0.01, 0.0]);
        assert_eq!(across.apply(&rutile).unwrap().structure, rutile);
        // The cell the atom is in does not matter; the member does.
        let image = AtomRef::new(O1, member, [1, -1, 2]);
        assert_eq!(
            drag(image, along).apply(&rutile).unwrap().structure,
            applied.structure
        );
        assert_eq!(
            drag(atom(O1, 4), along).apply(&rutile).unwrap_err(),
            EditError::UnknownAtom(atom(O1, 4))
        );
    }

    #[test]
    fn the_projection_is_the_nearest_allowed_displacement() {
        // 6h of P6₃/mmc, x, 2x, 1/4, runs along Cartesian y. P_s projects
        // orthogonally in Cartesian space, though not in fractional
        // coordinates (design §5.2).
        let s = hexagonal([0.17, 0.34, 0.25]);
        let gesture = [0.1, 0.05, 0.2];
        let applied = round_trip(&s, drag(atom(SiteId(1), 0), to_fractional(&s, gesture)));
        let before = s.lattice.to_cartesian(position(&s, SiteId(1)));
        let after = applied
            .structure
            .lattice
            .to_cartesian(position(&applied.structure, SiteId(1)));
        assert_close([0, 1, 2].map(|i| after[i] - before[i]), [0.0, 0.05, 0.0]);
    }

    #[test]
    fn leaving_a_special_position_raises_the_multiplicity() {
        let rutile = rutile();
        let applied = round_trip(&rutile, release(atom(O1, 0), [0.02, -0.03, 0.05]));
        assert_close(position(&applied.structure, O1), [X + 0.02, X - 0.03, 0.05]);
        assert_eq!(
            applied.info,
            [EditInfo::Multiplicity {
                site: O1,
                before: 4,
                after: 16
            }]
        );
        // 0.148 Å off the mirror z = 0: its images there are too close,
        // which warns but is allowed (design §7.4).
        let [EditWarning::CloseImages { site, distance }] = applied.warnings[..] else {
            panic!("{:?}", applied.warnings);
        };
        assert_eq!(site, O1);
        assert!((distance - 0.1 * 2.959).abs() < 1e-9, "{distance}");
        // References to its atoms keep only the site (design §7.11).
        assert_eq!(applied.ids.atom(atom(O1, 1)), AtomFate::Site(O1));
        assert_eq!(applied.ids.atom(atom(TI1, 0)), AtomFate::Kept(atom(TI1, 0)));
    }

    #[test]
    fn leaving_by_less_than_the_tolerance_snaps_back() {
        // 0.003 Å off the mirror: back on 4f, so nothing changes.
        let rutile = rutile();
        let applied = release(atom(O1, 0), [0.0, 0.0, 0.001])
            .apply(&rutile)
            .unwrap();
        assert_eq!(applied.structure, rutile);
        assert!(applied.info.is_empty());
    }

    #[test]
    fn a_site_moved_to_another_special_position_of_the_same_size_is_reshaped() {
        // From 4f (x, x, 0) to 4g (x, −x, 0): four atoms either way, but
        // from other operations, so member numbers mean other atoms.
        let rutile = rutile();
        let applied = round_trip(&rutile, release(atom(O1, 0), [0.0, 1.0 - 2.0 * X, 0.0]));
        assert!(applied.info.is_empty());
        assert_eq!(applied.ids.atom(atom(O1, 2)), AtomFate::Site(O1));
        let derived = Derived::compute(&applied.structure);
        let form = derived.site_symmetry(O1).unwrap().form.to_string();
        assert_eq!(form, "x, -x, 0");
    }

    #[test]
    fn atoms_dragged_near_a_special_position_snap_onto_it() {
        // A general position dragged to 0.0015 Å from the mirror z = 0.
        let s = rutile_with(Site::single(1, "O1", "O", [0.2, 0.1, 0.3]));
        let id = SiteId(1);
        let applied = round_trip(&s, drag(atom(id, 0), [0.0, 0.0, -0.2995]));
        assert_eq!(position(&applied.structure, id), [0.2, 0.1, 0.0]);
        let [
            EditInfo::Snapped { displacement, .. },
            EditInfo::Multiplicity { before, after, .. },
        ] = applied.info[..]
        else {
            panic!("{:?}", applied.info);
        };
        assert!(
            (displacement - 0.0005 * 2.959).abs() < 1e-9,
            "{displacement}"
        );
        assert_eq!((before, after), (16, 8));
        assert_eq!(applied.ids.atom(atom(id, 3)), AtomFate::Site(id));

        // Along its line, 4f reaches 2a at the origin, where Ti1 is: O1
        // snaps there and lands on Ti1, which only warns.
        let rutile = rutile();
        let applied = round_trip(&rutile, drag(atom(O1, 0), [0.0002 - X, 0.0002 - X, 0.0]));
        assert_eq!(position(&applied.structure, O1), [0.0, 0.0, 0.0]);
        assert!(matches!(
            applied.info[..],
            [
                EditInfo::Snapped { .. },
                EditInfo::Multiplicity {
                    before: 4,
                    after: 2,
                    ..
                }
            ]
        ));
        assert!(matches!(
            applied.warnings[..],
            [EditWarning::ShortContact { site: O1, other: TI1, distance }] if distance < 1e-9
        ));
    }

    #[test]
    fn a_drag_is_one_undo_step() {
        let rutile = rutile();
        let mut document = CrystalDocument::new(rutile.clone());
        let outcome = document
            .edit(release(atom(O1, 0), [0.0, 0.0, 0.1]))
            .unwrap();
        assert!(outcome.changed);
        assert_eq!(outcome.info.len(), 1);
        assert_eq!(document.derived().multiplicity(O1), Some(8));
        let undone = document.undo().unwrap();
        assert_eq!(document.structure(), &rutile);
        // Undoing reports the way back.
        assert_eq!(
            undone.info,
            [EditInfo::Multiplicity {
                site: O1,
                before: 8,
                after: 4
            }]
        );
        document.redo().unwrap();
        assert_close(position(document.structure(), O1), [X, X, 0.1]);
        assert_eq!(document.history().count(), 1);
    }

    #[test]
    fn moved_coordinates_lose_their_uncertainty_until_undone() {
        let site = Site::single(1, "O1", "O", [X, X, 0.0]).with_position_su([
            Some(2e-4),
            Some(2e-4),
            Some(0.0),
        ]);
        let s = rutile_with(site);
        let applied = round_trip(&s, drag(atom(SiteId(1), 0), [0.01, 0.01, 0.0]));
        assert_eq!(
            applied.structure.sites[0].position_su,
            [None, None, Some(0.0)]
        );
    }

    #[test]
    fn free_and_following_coordinates_can_be_typed() {
        // 6h, x, 2x, 1/4: typing x moves y with it, typing y moves x.
        let s = hexagonal([0.17, 0.34, 0.25]);
        let id = SiteId(1);
        let set = |axis, value| Edit::SetCoordinate {
            site: id,
            axis,
            value,
        };
        let applied = round_trip(&s, set(Axis::X, 0.2));
        assert_eq!(position(&applied.structure, id), [0.2, 0.4, 0.25]);
        let applied = round_trip(&s, set(Axis::Y, 0.5));
        assert_eq!(position(&applied.structure, id), [0.25, 0.5, 0.25]);
        assert!(applied.ids.is_identity() && applied.info.is_empty());
        // The constant is read only (design §7.5); its own value is fine.
        assert_eq!(
            set(Axis::Z, 0.3).apply(&s).unwrap_err(),
            EditError::FixedCoordinate {
                site: id,
                axis: Axis::Z
            }
        );
        assert_eq!(set(Axis::Z, 0.25).apply(&s).unwrap().structure, s);
        assert_eq!(
            set(Axis::X, f64::NAN).apply(&s).unwrap_err(),
            EditError::InvalidPosition
        );

        // A typed value is kept exactly, also where the projector would
        // round it: 4f of rutile.
        let rutile = rutile();
        let applied = round_trip(
            &rutile,
            Edit::SetCoordinate {
                site: O1,
                axis: Axis::Y,
                value: 0.3125,
            },
        );
        assert_eq!(position(&applied.structure, O1), [0.3125, 0.3125, 0.0]);
    }

    #[test]
    fn following_coordinates_keep_their_lattice_translation() {
        // x, −x, 1/2 at (0.2, 0.8, 0.5): y stays in [0, 1), next to 0.8.
        let s = rutile_with(Site::single(1, "O1", "O", [0.2, 0.8, 0.5]));
        let applied = round_trip(
            &s,
            Edit::SetCoordinate {
                site: SiteId(1),
                axis: Axis::X,
                value: 0.25,
            },
        );
        assert_eq!(position(&applied.structure, SiteId(1)), [0.25, 0.75, 0.5]);
    }

    #[test]
    fn typed_coordinates_snap_onto_more_symmetric_positions() {
        // x of 4f typed close to 0 puts O1 on 2a.
        let rutile = rutile();
        let set = |value| Edit::SetCoordinate {
            site: O1,
            axis: Axis::X,
            value,
        };
        let applied = round_trip(&rutile, set(0.0001));
        assert_eq!(position(&applied.structure, O1), [0.0, 0.0, 0.0]);
        assert!(matches!(
            applied.info[..],
            [
                EditInfo::Snapped { .. },
                EditInfo::Multiplicity {
                    before: 4,
                    after: 2,
                    ..
                }
            ]
        ));
        // Typed exactly, there is nothing to snap.
        let applied = round_trip(&rutile, set(0.0));
        assert!(matches!(
            applied.info[..],
            [EditInfo::Multiplicity { after: 2, .. }]
        ));
    }

    #[test]
    fn combined_coordinates_follow_but_cannot_be_typed() {
        // A mirror whose plane is z = x + y, as a non-standard setting
        // might give: x, y, x+y.
        let s = structure(
            &["x,y,2x+2y-z"],
            CellParams::new(4.0, 4.0, 4.0, 90.0, 90.0, 90.0),
            vec![Site::single(1, "O1", "O", [0.1, 0.2, 0.3])],
        );
        let id = SiteId(1);
        let form = Derived::compute(&s).site_symmetry(id).unwrap().form.clone();
        assert_eq!(form.to_string(), "x, y, x+y");
        let set = |axis, value| Edit::SetCoordinate {
            site: id,
            axis,
            value,
        };
        assert_eq!(
            set(Axis::Z, 0.4).apply(&s).unwrap_err(),
            EditError::CombinedCoordinate {
                site: id,
                axis: Axis::Z
            }
        );
        let applied = round_trip(&s, set(Axis::X, 0.15));
        assert_close(position(&applied.structure, id), [0.15, 0.2, 0.35]);
    }

    #[test]
    fn coordinates_in_p1_are_all_free() {
        let supercell = sample::load(SampleKey::RutileSupercell).structure;
        let id = supercell.sites[0].id;
        let mut expected = position(&supercell, id);
        expected[2] = 0.123;
        let applied = round_trip(
            &supercell,
            Edit::SetCoordinate {
                site: id,
                axis: Axis::Z,
                value: 0.123,
            },
        );
        assert_eq!(position(&applied.structure, id), expected);
    }

    #[test]
    fn positions_set_under_symmetry_take_the_new_site_symmetry() {
        let rutile = rutile();
        let set = |position| Edit::SetPosition { site: O1, position };
        // Still on 4f: references stay.
        let applied = round_trip(&rutile, set([0.3, 0.3, 0.0]));
        assert!(applied.ids.is_identity() && applied.info.is_empty());
        // Off the line but on the mirror: 8i.
        let applied = round_trip(&rutile, set([0.31, 0.29, 0.0]));
        assert_eq!(
            applied.info,
            [EditInfo::Multiplicity {
                site: O1,
                before: 4,
                after: 8
            }]
        );
        // 0.0015 Å from the mirror: snapped onto it, and reported.
        let applied = round_trip(&rutile, set([0.3, 0.3, 0.0005]));
        assert_eq!(position(&applied.structure, O1), [0.3, 0.3, 0.0]);
        assert!(matches!(
            applied.info[..],
            [EditInfo::Snapped { site: O1, .. }]
        ));
    }

    fn add(position: [f64; 3], element: &str, label: Option<&str>) -> Edit {
        Edit::AddSite {
            position,
            species: Species::element(element),
            occupancy: Measured::exact(1.0),
            label: label.map(str::to_string),
        }
    }

    #[test]
    fn added_sites_snap_to_the_average_of_their_images() {
        // 0.0013 Å off 4f in the plane, 0.003 Å along c (design §7.6).
        let rutile = rutile();
        let applied = round_trip(&rutile, add([0.2046, 0.2050, 0.001], "O", None));
        let added = applied.structure.sites.last().unwrap();
        assert_eq!(added.id, SiteId(3));
        assert_eq!(added.label(), "O2");
        assert_close(added.position, [0.2048, 0.2048, 0.0]);
        let [EditInfo::Snapped { site, displacement }] = applied.info[..] else {
            panic!("{:?}", applied.info);
        };
        assert_eq!(site, SiteId(3));
        assert!((displacement - 0.00323).abs() < 1e-5, "{displacement}");
        assert!(applied.warnings.is_empty() && applied.ids.is_identity());
        let derived = Derived::compute(&applied.structure);
        assert_eq!(derived.multiplicity(SiteId(3)), Some(4));

        // Next to an image of 2b in another cell: it stays in that cell.
        let applied = round_trip(&rutile, add([0.0003, 0.9998, 1.4999], "Zr", None));
        let added = applied.structure.sites.last().unwrap();
        assert_eq!(added.position, [0.0, 1.0, 1.5]);
        assert_eq!(added.label(), "Zr1");
    }

    #[test]
    fn added_sites_must_not_coincide_with_an_orbit() {
        let rutile = rutile();
        // Another atom of O1's orbit, within the tolerance.
        for position in [[1.0 - X, 1.0 - X, 0.0], [0.6953, 0.6951, 0.0005]] {
            let error = add(position, "F", None).apply(&rutile).unwrap_err();
            assert_eq!(error, EditError::CoincidentSite { site: O1 });
            assert_eq!(error.guidance(), [Guidance::AddOccupant { site: O1 }]);
        }
        assert_eq!(
            add([1.0, 0.0, -1.0], "Zr", None)
                .apply(&rutile)
                .unwrap_err(),
            EditError::CoincidentSite { site: TI1 }
        );
        // Close to Ti1, and its images to each other, but not on them:
        // added with warnings (design §7.4).
        let applied = add([0.05, 0.0, 0.0], "H", None).apply(&rutile).unwrap();
        assert!(matches!(
            applied.warnings[..],
            [
                EditWarning::CloseImages { .. },
                EditWarning::ShortContact { other: TI1, distance, .. }
            ] if (distance - 0.05 * 4.594).abs() < 1e-9
        ));
    }

    #[test]
    fn added_sites_are_checked_and_labeled() {
        let rutile = rutile();
        let first = add([0.2, 0.2, 0.0], "O", None).apply(&rutile).unwrap();
        let second = add([0.1, 0.4, 0.0], "O", None)
            .apply(&first.structure)
            .unwrap();
        let labels: Vec<&str> = second.structure.sites.iter().map(Site::label).collect();
        assert_eq!(labels, ["Ti1", "O1", "O2", "O3"]);
        assert_eq!(second.structure.sites[3].id, SiteId(4));

        let refused = |edit: Edit| edit.apply(&rutile).unwrap_err();
        assert_eq!(
            refused(add([0.2, 0.2, 0.0], "O", Some("Ti1"))),
            EditError::DuplicateLabel {
                label: "Ti1".into()
            }
        );
        assert_eq!(
            refused(add([0.2, 0.2, 0.0], "Qq", None)),
            EditError::UnknownElement {
                symbol: "Qq".into()
            }
        );
        assert_eq!(
            refused(add([0.2, f64::INFINITY, 0.0], "O", None)),
            EditError::InvalidPosition
        );
        let empty = Edit::AddSite {
            position: [0.2, 0.2, 0.0],
            species: Species::element("O"),
            occupancy: Measured::exact(0.0),
            label: None,
        };
        assert!(matches!(
            refused(empty),
            EditError::OccupancyOutOfRange { .. }
        ));

        // Undo and redo bring back the same site.
        let mut document = CrystalDocument::new(rutile.clone());
        document.edit(add([0.2, 0.2, 0.0], "O", None)).unwrap();
        let added = document.structure().clone();
        document.undo().unwrap();
        assert_eq!(document.structure(), &rutile);
        document.redo().unwrap();
        assert_eq!(document.structure(), &added);
    }

    #[test]
    fn geometric_edits_serialize_as_tagged_json() {
        let edits = [
            release(AtomRef::new(O1, 2, [0, 1, 0]), [0.01, 0.0, -0.02]),
            Edit::SetCoordinate {
                site: O1,
                axis: Axis::Y,
                value: 0.31,
            },
            Edit::RestorePosition {
                site: O1,
                position: [X, X, 0.0],
                position_su: [Some(2e-4), None, None],
            },
            add([0.2, 0.2, 0.0], "O", Some("O7")),
        ];
        for edit in edits {
            let json = serde_json::to_string(&edit).unwrap();
            assert_eq!(serde_json::from_str::<Edit>(&json).unwrap(), edit, "{json}");
        }
        let json = serde_json::to_value(Edit::SetCoordinate {
            site: O1,
            axis: Axis::Z,
            value: 0.5,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"command": "set_coordinate", "site": 2, "axis": "z", "value": 0.5})
        );
        // Dragging stays on the special position unless asked otherwise.
        let edit: Edit = serde_json::from_value(serde_json::json!({
            "command": "move_site",
            "atom": {"site": 2, "member": 0, "cell": [0, 0, 0]},
            "displacement": [0.01, 0.0, 0.0]
        }))
        .unwrap();
        assert_eq!(edit, drag(atom(O1, 0), [0.01, 0.0, 0.0]));
    }
}
