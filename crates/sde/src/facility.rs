//! Where a job is built: an NPC station or a structure, its security and the rigs fitted to it.
//! Turns that set-up into the exact material modifier for each product.

use std::collections::BTreeSet;
use std::fmt;

use profit_core::industry::{Activity, IndustryError, Modifier, TypeId};
use rusqlite::types::Type;
use rusqlite::{OptionalExtension, params};

use crate::error::SdeError;
use crate::query::{Item, Sde};
use crate::schema::{activity_from_name, activity_name};

/// The group of engineering complexes: manufacturing only.
pub(crate) const ENGINEERING_COMPLEX_GROUP: u32 = 1404;
/// The group of refineries: reactions only, in low or null security.
pub(crate) const REFINERY_GROUP: u32 = 1406;
/// The most rigs one structure takes.
const MAX_RIGS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    High,
    Low,
    Null,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructureKind {
    EngineeringComplex,
    Refinery,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Structure {
    pub type_id: TypeId,
    pub name: String,
    pub kind: StructureKind,
    pub rig_size: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rig {
    pub type_id: TypeId,
    pub name: String,
    pub activity: Activity,
    /// Material bonus in tenths of a percent, negative: -24 is -2.4%.
    pub bonus_tenths_percent: i32,
    pub rig_size: u8,
}

/// Where a job is built. `structure: None` is an NPC station.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facility {
    pub structure: Option<TypeId>,
    pub security: Security,
    pub rigs: Vec<TypeId>,
}

/// Why a set-up or a job cannot be built as given.
#[derive(Debug)]
pub enum FacilityError {
    NotAStructure(TypeId),
    ActivityNotAvailable {
        activity: Activity,
        structure: Option<TypeId>,
        security: Security,
    },
    TooManyRigs(usize),
    NotARig(TypeId),
    /// The rig is for another structure size or kind, or is for another activity.
    RigDoesNotFit {
        rig: TypeId,
        structure: Option<TypeId>,
    },
    /// A second rig of a group that is already fitted.
    RigGroupTwice(TypeId),
    MeOnReaction(u8),
    UnknownProduct(TypeId),
    Industry(IndustryError),
    Sde(SdeError),
}

impl fmt::Display for FacilityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAStructure(id) => {
                write!(f, "type {id} is not an engineering complex or refinery")
            }
            Self::ActivityNotAvailable {
                activity,
                structure,
                security,
            } => write!(
                f,
                "the {} activity is not offered at {} in {} security space",
                activity_name(*activity),
                place(*structure),
                security_word(*security)
            ),
            Self::TooManyRigs(count) => {
                write!(
                    f,
                    "{count} rigs are given, but a structure takes at most {MAX_RIGS}"
                )
            }
            Self::NotARig(id) => write!(f, "type {id} is not a rig"),
            Self::RigDoesNotFit { rig, structure } => {
                write!(f, "rig {rig} does not fit {}", place(*structure))
            }
            Self::RigGroupTwice(rig) => {
                write!(
                    f,
                    "rig {rig} is a second rig of a group that is already fitted"
                )
            }
            Self::MeOnReaction(me) => {
                write!(
                    f,
                    "reactions take no material efficiency, so ME {me} is not allowed"
                )
            }
            Self::UnknownProduct(id) => write!(f, "type {id} is not a known product"),
            Self::Industry(error) => write!(f, "{error}"),
            Self::Sde(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for FacilityError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Industry(error) => Some(error),
            Self::Sde(error) => Some(error),
            Self::NotAStructure(_)
            | Self::ActivityNotAvailable { .. }
            | Self::TooManyRigs(_)
            | Self::NotARig(_)
            | Self::RigDoesNotFit { .. }
            | Self::RigGroupTwice(_)
            | Self::MeOnReaction(_)
            | Self::UnknownProduct(_) => None,
        }
    }
}

impl From<IndustryError> for FacilityError {
    fn from(error: IndustryError) -> Self {
        Self::Industry(error)
    }
}

impl From<SdeError> for FacilityError {
    fn from(error: SdeError) -> Self {
        Self::Sde(error)
    }
}

fn place(structure: Option<TypeId>) -> String {
    match structure {
        Some(id) => format!("structure {id}"),
        None => "an NPC station".to_owned(),
    }
}

fn security_word(security: Security) -> &'static str {
    match security {
        Security::High => "high",
        Security::Low => "low",
        Security::Null => "null",
    }
}

fn kind_of(group_id: u32) -> Option<StructureKind> {
    match group_id {
        ENGINEERING_COMPLEX_GROUP => Some(StructureKind::EngineeringComplex),
        REFINERY_GROUP => Some(StructureKind::Refinery),
        _ => None,
    }
}

/// A structure as the database stores it.
struct StructureInfo {
    group_id: u32,
    kind: StructureKind,
    rig_size: u8,
    /// Manufacturing material bonus in hundredths. `None` when the structure has none.
    manufacturing_bonus: Option<u32>,
}

/// A rig as the database stores it.
struct RigInfo {
    group_id: u32,
    activity: Activity,
    bonus_tenths: i32,
    rig_size: u8,
    /// `None` in the list covers every product.
    filter_ids: Vec<Option<TypeId>>,
    /// `None` for a reaction rig, which does not work in high security.
    high_tenths: Option<u32>,
    low_tenths: u32,
    null_tenths: u32,
}

/// A rig that a set-up applies to a job, in the order the set-up lists them.
struct AppliedRig {
    bonus_tenths: i32,
    filter_ids: Vec<Option<TypeId>>,
    security_tenths: u32,
}

/// What a set-up applies to a job, once its checks pass.
struct Setup {
    structure_bonus: Option<u32>,
    rigs: Vec<AppliedRig>,
}

impl Sde {
    /// The engineering complexes and refineries the app offers, by type id.
    pub fn structures(&self) -> Result<Vec<Structure>, SdeError> {
        let mut statement = self.conn.prepare(
            "SELECT s.type_id, t.name, t.group_id, s.rig_size
             FROM structures s JOIN types t ON t.type_id = s.type_id
             ORDER BY s.type_id",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, TypeId>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u32>(2)?,
                    row.get::<_, u8>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows
            .into_iter()
            .filter_map(|(type_id, name, group_id, rig_size)| {
                Some(Structure {
                    type_id,
                    name,
                    kind: kind_of(group_id)?,
                    rig_size,
                })
            })
            .collect())
    }

    /// The rigs that fit `structure` and give a material bonus for `activity`, by type id.
    pub fn rigs_for(
        &self,
        structure: TypeId,
        activity: Activity,
    ) -> Result<Vec<Rig>, FacilityError> {
        let info = self
            .structure_info(structure)?
            .ok_or(FacilityError::NotAStructure(structure))?;
        Ok(self.fitting_rigs(&info, activity)?)
    }

    /// The material modifier of `product` built in `facility` for `activity` at `me`: the ME, then
    /// the structure bonus, then each rig that covers the product, in the order given.
    pub fn modifier(
        &self,
        facility: &Facility,
        activity: Activity,
        product: TypeId,
        me: u8,
    ) -> Result<Modifier, FacilityError> {
        let setup = self.setup(facility, activity)?;
        let mut modifier = Modifier::one();
        match activity {
            Activity::Manufacturing => modifier = modifier.with_me(me)?,
            Activity::Reaction if me != 0 => return Err(FacilityError::MeOnReaction(me)),
            Activity::Reaction => {}
        }
        let item = self
            .item(product)?
            .ok_or(FacilityError::UnknownProduct(product))?;
        if let Some(hundredths) = setup.structure_bonus {
            modifier = modifier.with_structure(hundredths)?;
        }
        for rig in setup.rigs {
            if self.covers(&rig.filter_ids, &item)? {
                modifier = modifier.with_rig(rig.bonus_tenths, rig.security_tenths)?;
            }
        }
        Ok(modifier)
    }

    /// The checks of `modifier` that need no product or ME: the set-up can be built for `activity`.
    pub fn check_facility(
        &self,
        facility: &Facility,
        activity: Activity,
    ) -> Result<(), FacilityError> {
        self.setup(facility, activity).map(|_| ())
    }

    /// Checks in order: the structure, the activity and security, the rig count, then each rig.
    fn setup(&self, facility: &Facility, activity: Activity) -> Result<Setup, FacilityError> {
        let structure = match facility.structure {
            Some(id) => Some(
                self.structure_info(id)?
                    .ok_or(FacilityError::NotAStructure(id))?,
            ),
            None => None,
        };
        let offered = match (activity, structure.as_ref().map(|s| s.kind)) {
            (Activity::Manufacturing, None | Some(StructureKind::EngineeringComplex)) => true,
            (Activity::Reaction, Some(StructureKind::Refinery)) => {
                facility.security != Security::High
            }
            _ => false,
        };
        if !offered {
            return Err(FacilityError::ActivityNotAvailable {
                activity,
                structure: facility.structure,
                security: facility.security,
            });
        }
        if facility.rigs.len() > MAX_RIGS {
            return Err(FacilityError::TooManyRigs(facility.rigs.len()));
        }

        let mut groups = BTreeSet::new();
        let mut rigs = Vec::with_capacity(facility.rigs.len());
        for &rig_id in &facility.rigs {
            let rig = self
                .rig_info(rig_id)?
                .ok_or(FacilityError::NotARig(rig_id))?;
            let fits = match &structure {
                Some(s) => {
                    s.rig_size == rig.rig_size
                        && rig.activity == activity
                        && self.rig_fits(rig_id, s.group_id)?
                }
                None => false,
            };
            if !fits {
                return Err(FacilityError::RigDoesNotFit {
                    rig: rig_id,
                    structure: facility.structure,
                });
            }
            if !groups.insert(rig.group_id) {
                return Err(FacilityError::RigGroupTwice(rig_id));
            }
            let security_tenths = match facility.security {
                Security::High => rig.high_tenths,
                Security::Low => Some(rig.low_tenths),
                Security::Null => Some(rig.null_tenths),
            };
            // Offered only where the rig has a multiplier: reactions are never in high security.
            let security_tenths = security_tenths.ok_or(FacilityError::ActivityNotAvailable {
                activity,
                structure: facility.structure,
                security: facility.security,
            })?;
            rigs.push(AppliedRig {
                bonus_tenths: rig.bonus_tenths,
                filter_ids: rig.filter_ids,
                security_tenths,
            });
        }

        let structure_bonus = match (activity, &structure) {
            (Activity::Manufacturing, Some(s)) => s.manufacturing_bonus,
            _ => None,
        };
        Ok(Setup {
            structure_bonus,
            rigs,
        })
    }

    fn structure_info(&self, id: TypeId) -> Result<Option<StructureInfo>, SdeError> {
        let row = self
            .conn
            .query_row(
                "SELECT t.group_id, s.rig_size, s.manufacturing_bonus_hundredths
                 FROM structures s JOIN types t ON t.type_id = s.type_id
                 WHERE s.type_id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, u32>(0)?,
                        row.get::<_, u8>(1)?,
                        row.get::<_, Option<u32>>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((group_id, rig_size, manufacturing_bonus)) = row else {
            return Ok(None);
        };
        let Some(kind) = kind_of(group_id) else {
            return Ok(None);
        };
        Ok(Some(StructureInfo {
            group_id,
            kind,
            rig_size,
            manufacturing_bonus,
        }))
    }

    fn rig_info(&self, id: TypeId) -> Result<Option<RigInfo>, SdeError> {
        let info = self
            .conn
            .query_row(
                "SELECT t.group_id, r.activity, r.bonus_tenths, r.rig_size,
                        r.high_tenths, r.low_tenths, r.null_tenths
                 FROM rigs r JOIN types t ON t.type_id = r.type_id
                 WHERE r.type_id = ?1",
                params![id],
                |row| {
                    let activity: String = row.get(1)?;
                    Ok(RigInfo {
                        group_id: row.get(0)?,
                        activity: activity_from_name(&activity).ok_or_else(|| {
                            rusqlite::Error::InvalidColumnType(1, activity.clone(), Type::Text)
                        })?,
                        bonus_tenths: row.get(2)?,
                        rig_size: row.get(3)?,
                        filter_ids: Vec::new(),
                        high_tenths: row.get(4)?,
                        low_tenths: row.get(5)?,
                        null_tenths: row.get(6)?,
                    })
                },
            )
            .optional()?;
        let Some(mut info) = info else {
            return Ok(None);
        };
        info.filter_ids = self.rig_filter_ids(id)?;
        Ok(Some(info))
    }

    fn rig_filter_ids(&self, rig_id: TypeId) -> Result<Vec<Option<TypeId>>, SdeError> {
        let mut statement = self
            .conn
            .prepare("SELECT filter_id FROM rig_filters WHERE rig_id = ?1")?;
        let filter_ids = statement
            .query_map(params![rig_id], |row| row.get::<_, Option<TypeId>>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(filter_ids)
    }

    fn rig_fits(&self, rig_id: TypeId, group_id: u32) -> Result<bool, SdeError> {
        let fits: bool = self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM rig_fits WHERE rig_id = ?1 AND group_id = ?2)",
            params![rig_id, group_id],
            |row| row.get(0),
        )?;
        Ok(fits)
    }

    /// Whether any of the filters covers the product. A `None` filter covers every product.
    fn covers(&self, filter_ids: &[Option<TypeId>], item: &Item) -> Result<bool, SdeError> {
        for filter_id in filter_ids {
            let covered = match filter_id {
                Some(filter) => self.filter_covers(*filter, item)?,
                None => true,
            };
            if covered {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Whether the filter names the product's category or its group.
    fn filter_covers(&self, filter_id: TypeId, item: &Item) -> Result<bool, SdeError> {
        let covers: bool = self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM filter_categories
                            WHERE filter_id = ?1 AND category_id = ?2)
                 OR EXISTS (SELECT 1 FROM filter_groups
                            WHERE filter_id = ?1 AND group_id = ?3)",
            params![filter_id, item.category_id, item.group_id],
            |row| row.get(0),
        )?;
        Ok(covers)
    }

    fn fitting_rigs(&self, info: &StructureInfo, activity: Activity) -> Result<Vec<Rig>, SdeError> {
        let mut statement = self.conn.prepare(
            "SELECT r.type_id, t.name, r.bonus_tenths
             FROM rigs r JOIN types t ON t.type_id = r.type_id
             WHERE r.activity = ?1 AND r.rig_size = ?2
               AND EXISTS (SELECT 1 FROM rig_fits f
                           WHERE f.rig_id = r.type_id AND f.group_id = ?3)
             ORDER BY r.type_id",
        )?;
        let rigs = statement
            .query_map(
                params![activity_name(activity), info.rig_size, info.group_id],
                |row| {
                    Ok(Rig {
                        type_id: row.get(0)?,
                        name: row.get(1)?,
                        activity,
                        bonus_tenths_percent: row.get(2)?,
                        rig_size: info.rig_size,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rigs)
    }
}
