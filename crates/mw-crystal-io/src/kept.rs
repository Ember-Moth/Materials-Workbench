//! The fields of a CIF data block that the structure does not hold, kept so
//! that a CIF export can write them back (design §6, CIF round trip).
//!
//! Every tag falls into one of four classes ([`Class`]): read into the
//! structure, derived from it, descriptive (publication, experiment,
//! refinement) or unrecognized. Only the last two are kept. The columns of
//! the atom site loops that the structure does not hold, such as
//! `_atom_site_calc_flag`, are kept per atom label so that they can be
//! written next to the atoms again.

use std::collections::{BTreeMap, HashSet};

use mw_crystal::Structure;

use crate::syntax::{Block, Item, Tag, Value};

/// How a CIF export treats a field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    /// Read into the structure and written from it.
    Model,
    /// Follows from the structure, e.g. `_cell_volume`, Wyckoff letters,
    /// `_geom_*`: not kept; the writer regenerates what it can.
    Derived,
    /// About the publication, the experiment and the refinement: written
    /// back as read, even after the structure changed.
    Descriptive,
    /// Everything else: written back while the structure is unchanged;
    /// dropped, and reported, once it changed.
    Unrecognized,
}

/// Tags the importer reads (see `import`). The writer writes all of them
/// that apply, under their current names.
const MODEL: &[&str] = &[
    "_cell_length_a",
    "_cell_length_b",
    "_cell_length_c",
    "_cell_angle_alpha",
    "_cell_angle_beta",
    "_cell_angle_gamma",
    "_space_group_symop_id",
    "_space_group_symop_operation_xyz",
    "_symmetry_equiv_pos_site_id",
    "_symmetry_equiv_pos_as_xyz",
    "_space_group_name_h-m_alt",
    "_symmetry_space_group_name_h-m",
    "_space_group_it_number",
    "_symmetry_int_tables_number",
    "_space_group_name_hall",
    "_symmetry_space_group_name_hall",
    "_atom_site_label",
    "_atom_site_type_symbol",
    "_atom_site_fract_x",
    "_atom_site_fract_y",
    "_atom_site_fract_z",
    "_atom_site_occupancy",
    "_atom_site_u_iso_or_equiv",
    "_atom_site_b_iso_or_equiv",
    "_atom_site_adp_type",
    "_atom_site_disorder_assembly",
    "_atom_site_disorder_group",
    "_atom_site_aniso_label",
    "_atom_site_aniso_u_11",
    "_atom_site_aniso_u_22",
    "_atom_site_aniso_u_33",
    "_atom_site_aniso_u_12",
    "_atom_site_aniso_u_13",
    "_atom_site_aniso_u_23",
    "_atom_site_aniso_b_11",
    "_atom_site_aniso_b_22",
    "_atom_site_aniso_b_33",
    "_atom_site_aniso_b_12",
    "_atom_site_aniso_b_13",
    "_atom_site_aniso_b_23",
];

/// Tags whose values follow from the structure.
const DERIVED: &[&str] = &[
    "_cell_volume",
    "_cell_formula_units_z",
    "_chemical_formula_sum",
    "_chemical_formula_weight",
    "_exptl_crystal_density_diffrn",
    "_exptl_crystal_f_000",
    "_space_group_crystal_system",
    "_symmetry_cell_setting",
    "_atom_site_symmetry_multiplicity",
    "_atom_site_site_symmetry_multiplicity",
    "_atom_site_site_symmetry_order",
    "_atom_site_wyckoff_symbol",
    "_atom_site_cartn_x",
    "_atom_site_cartn_y",
    "_atom_site_cartn_z",
    "_atom_site_aniso_type_symbol",
    // The CIF 1.0 name of `_atom_site_adp_type`, which the writer writes.
    "_atom_site_thermal_displace_type",
];

/// Categories whose fields are derived, as tag prefixes.
const DERIVED_PREFIXES: &[&str] = &["_geom_"];

/// Categories that describe where the data came from, as tag prefixes.
const DESCRIPTIVE_PREFIXES: &[&str] = &[
    "_audit_",
    "_publ_",
    "_journal_",
    "_citation_",
    "_database_",
    "_cod_",
    "_chemical_name_",
    "_cell_measurement_",
    "_exptl_",
    "_diffrn_",
    "_reflns_",
    "_refine_",
    "_computing_",
    "_pd_",
];

pub(crate) fn classify(tag: &str) -> Class {
    if MODEL.contains(&tag) {
        Class::Model
    } else if DERIVED.contains(&tag) || DERIVED_PREFIXES.iter().any(|p| tag.starts_with(p)) {
        Class::Derived
    } else if DESCRIPTIVE_PREFIXES.iter().any(|p| tag.starts_with(p)) {
        Class::Descriptive
    } else {
        Class::Unrecognized
    }
}

/// A value as the file wrote it: unquoted values stay unquoted, so that
/// `?` and `.` keep their meaning. Quotes a value does not need are not
/// remembered, so that a written and reread value equals the original.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Text {
    pub text: String,
    pub quoted: bool,
}

impl From<&Value> for Text {
    fn from(value: &Value) -> Self {
        Self {
            text: value.text.clone(),
            quoted: value.quoted && !crate::cif_write::is_bare(&value.text),
        }
    }
}

/// A kept pair or loop, tags spelled as in the file.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Field {
    Pair {
        tag: String,
        value: Text,
    },
    Loop {
        tags: Vec<String>,
        values: Vec<Text>,
    },
}

impl Field {
    pub fn tags(&self) -> impl Iterator<Item = &str> {
        let tags: &[String] = match self {
            Self::Pair { tag, .. } => std::slice::from_ref(tag),
            Self::Loop { tags, .. } => tags,
        };
        tags.iter().map(String::as_str)
    }
}

/// A column of an atom site loop that the structure does not hold, e.g.
/// `_atom_site_calc_flag`: its value for each atom label.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Column {
    pub tag: String,
    pub values: BTreeMap<String, Text>,
}

/// The fields of a data block beyond the structure, for writing it back
/// with [`crate::cif::write`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CifFields {
    /// Descriptive and unrecognized pairs and loops, in file order.
    pub(crate) fields: Vec<(Class, Field)>,
    /// Unrecognized columns of `_atom_site_`.
    pub(crate) site_columns: Vec<Column>,
    /// Unrecognized columns of `_atom_site_aniso_`.
    pub(crate) aniso_columns: Vec<Column>,
}

impl CifFields {
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty() && self.site_columns.is_empty() && self.aniso_columns.is_empty()
    }

    /// Forgets the column values of atoms the structure does not have, such
    /// as skipped dummy atoms and dropped repeated rows: they are never
    /// written.
    pub(crate) fn retain_atoms(&mut self, structure: &Structure) {
        let labels: HashSet<&str> = structure
            .sites
            .iter()
            .flat_map(|site| &site.occupants)
            .map(|o| o.label.as_str())
            .collect();
        for column in self.site_columns.iter_mut().chain(&mut self.aniso_columns) {
            column
                .values
                .retain(|label, _| labels.contains(label.as_str()));
        }
    }
}

/// The tag as the file spells it: tags are compared in lowercase, but
/// written back as they were.
fn spelling(tag: &Tag, lines: &[&str]) -> String {
    lines
        .get(tag.line as usize - 1)
        .and_then(|line| {
            line.split_whitespace()
                .find(|word| word.eq_ignore_ascii_case(&tag.name))
        })
        .map_or_else(|| tag.name.clone(), str::to_string)
}

/// The kept fields of a block. `lines` are the lines of the file, for the
/// spelling of the tags.
pub(crate) fn collect(block: &Block, lines: &[&str]) -> CifFields {
    let mut kept = CifFields::default();
    // Atom sites written as pairs: one row, keyed by the label pair.
    let mut site_pairs: Vec<(&Tag, &Value)> = Vec::new();
    let mut aniso_pairs: Vec<(&Tag, &Value)> = Vec::new();
    for item in &block.items {
        match item {
            Item::Pair { tag, value } if tag.name.starts_with("_atom_site_aniso_") => {
                aniso_pairs.push((tag, value));
            }
            Item::Pair { tag, value } if tag.name.starts_with("_atom_site_") => {
                site_pairs.push((tag, value));
            }
            Item::Pair { tag, value } => match classify(&tag.name) {
                class @ (Class::Descriptive | Class::Unrecognized) => kept.fields.push((
                    class,
                    Field::Pair {
                        tag: spelling(tag, lines),
                        value: value.into(),
                    },
                )),
                Class::Model | Class::Derived => {}
            },
            Item::Loop(table) => {
                if let Some(label) = table.column("_atom_site_aniso_label") {
                    let rows = table.rows().map(|row| (&row[label], row));
                    kept.aniso_columns.extend(columns(&table.tags, rows, lines));
                } else if let Some(label) = table.column("_atom_site_label") {
                    let rows = table.rows().map(|row| (&row[label], row));
                    kept.site_columns.extend(columns(&table.tags, rows, lines));
                } else {
                    // A loop belongs to one category; one regenerated or
                    // derived column means the writer has its own version.
                    let classes: Vec<Class> =
                        table.tags.iter().map(|tag| classify(&tag.name)).collect();
                    if classes.contains(&Class::Model) || classes.contains(&Class::Derived) {
                        continue;
                    }
                    let class = if classes.contains(&Class::Unrecognized) {
                        Class::Unrecognized
                    } else {
                        Class::Descriptive
                    };
                    kept.fields.push((
                        class,
                        Field::Loop {
                            tags: table.tags.iter().map(|tag| spelling(tag, lines)).collect(),
                            values: table.values.iter().map(Text::from).collect(),
                        },
                    ));
                }
            }
        }
    }
    for (pairs, label_tag, columns_out) in [
        (&site_pairs, "_atom_site_label", &mut kept.site_columns),
        (
            &aniso_pairs,
            "_atom_site_aniso_label",
            &mut kept.aniso_columns,
        ),
    ] {
        let Some((_, label)) = pairs.iter().find(|(tag, _)| tag.name == label_tag) else {
            continue;
        };
        let tags: Vec<Tag> = pairs.iter().map(|(tag, _)| (*tag).clone()).collect();
        let row: Vec<Value> = pairs.iter().map(|(_, value)| (*value).clone()).collect();
        columns_out.extend(columns(&tags, [(*label, &row[..])], lines));
    }
    kept
}

/// The unrecognized columns of an atom site loop, keyed by label.
fn columns<'a>(
    tags: &[Tag],
    rows: impl IntoIterator<Item = (&'a Value, &'a [Value])>,
    lines: &[&str],
) -> Vec<Column> {
    let indices: Vec<usize> = (0..tags.len())
        .filter(|&i| classify(&tags[i].name) == Class::Unrecognized)
        .collect();
    let mut columns: Vec<Column> = indices
        .iter()
        .map(|&i| Column {
            tag: spelling(&tags[i], lines),
            values: BTreeMap::new(),
        })
        .collect();
    for (label, row) in rows {
        for (column, &i) in columns.iter_mut().zip(&indices) {
            if let Some(value) = row.get(i) {
                column.values.insert(label.text.clone(), value.into());
            }
        }
    }
    columns
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(text: &str) -> CifFields {
        let document = crate::syntax::parse(text);
        let lines: Vec<&str> = text.lines().collect();
        collect(&document.blocks[0], &lines)
    }

    #[test]
    fn classes() {
        assert_eq!(classify("_cell_length_a"), Class::Model);
        assert_eq!(classify("_cell_volume"), Class::Derived);
        assert_eq!(classify("_geom_bond_distance"), Class::Derived);
        assert_eq!(classify("_journal_name_full"), Class::Descriptive);
        assert_eq!(classify("_exptl_crystal_density_diffrn"), Class::Derived);
        assert_eq!(classify("_exptl_crystal_colour"), Class::Descriptive);
        assert_eq!(classify("_atom_type_symbol"), Class::Unrecognized);
        assert_eq!(classify("_chemical_formula_moiety"), Class::Unrecognized);
    }

    #[test]
    fn keeps_what_the_structure_does_not_hold() {
        let kept = fields(
            "data_x\n\
             _Journal_Name_Full 'Acta Cryst.'\n\
             _cell_length_a 4\n\
             _cell_volume 64\n\
             _Custom_Note ?\n\
             loop_\n_atom_type_symbol _atom_type_oxidation_number\nTi4+ 4\nO2- -2\n\
             loop_\n_space_group_symop_id _space_group_symop_operation_xyz _x_extra\n1 x,y,z a\n\
             loop_\n_atom_site_label _atom_site_fract_x _atom_site_Calc_Flag\nTi1 0 d\nO1 0.3 calc\n",
        );
        let tags: Vec<(Class, Vec<&str>)> = kept
            .fields
            .iter()
            .map(|(class, field)| (*class, field.tags().collect()))
            .collect();
        assert_eq!(
            tags,
            [
                (Class::Descriptive, vec!["_Journal_Name_Full"]),
                (Class::Unrecognized, vec!["_Custom_Note"]),
                (
                    Class::Unrecognized,
                    vec!["_atom_type_symbol", "_atom_type_oxidation_number"]
                ),
            ]
        );
        let Field::Pair { value, .. } = &kept.fields[1].1 else {
            panic!("pair");
        };
        assert!(!value.quoted, "`?` stays the unknown value");
        assert_eq!(kept.site_columns.len(), 1);
        let column = &kept.site_columns[0];
        assert_eq!(column.tag, "_atom_site_Calc_Flag");
        assert_eq!(column.values["O1"].text, "calc");
    }

    #[test]
    fn atom_sites_written_as_pairs() {
        let kept = fields(
            "data_x\n_atom_site_label Na1\n_atom_site_fract_x 0\n_atom_site_refinement_flags P\n",
        );
        assert_eq!(kept.site_columns[0].values["Na1"].text, "P");
    }
}
