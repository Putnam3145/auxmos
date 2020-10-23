pub const R_IDEAL_GAS_EQUATION: f32 = 8.31; //kPa*L/(K*mol)
pub const ONE_ATMOSPHERE: f32 = 101.325; //kPa
pub const TCMB: f32 = 2.7; // -270.3degC
pub const TCRYO: f32 = 225.0; // -48.15degC
pub const T0C: f32 = 273.15; // 0degC
pub const T20C: f32 = 293.15; // 20degC

pub const GAS_MIN_MOLES: f32 = 0.00000005;

pub const MINIMUM_HEAT_CAPACITY: f32 = 0.0003;

pub const CELL_VOLUME: f32 = 2500.0; //liters in a cell
pub const MOLES_CELLSTANDARD: f32 = ONE_ATMOSPHERE * CELL_VOLUME / (T20C * R_IDEAL_GAS_EQUATION); //moles in a 2.5 m^3 cell at 101.325 Pa and 20 degC
pub const M_CELL_WITH_RATIO: f32 = MOLES_CELLSTANDARD * 0.005; //compared against for superconductivity
pub const O2STANDARD: f32 = 0.21; //percentage of oxygen in a normal mixture of air
pub const N2STANDARD: f32 = 0.79; //same but for nitrogen
pub const MOLES_O2STANDARD: f32 = MOLES_CELLSTANDARD * O2STANDARD; // O2 standard value (21%)
pub const MOLES_N2STANDARD: f32 = MOLES_CELLSTANDARD * N2STANDARD; // N2 standard value (79%)
pub const BREATH_VOLUME: f32 = 0.5; //liters in a normal breath
pub const BREATH_PERCENTAGE: f32 = BREATH_VOLUME / CELL_VOLUME; //Amount of air to take a from a tile

//EXCITED GROUPS
pub const EXCITED_GROUP_BREAKDOWN_CYCLES: i32 = 4; //number of FULL air controller ticks before an excited group breaks down (averages gas contents across turfs)
pub const EXCITED_GROUP_DISMANTLE_CYCLES: i32 = 16; //number of FULL air controller ticks before an excited group dismantles and removes its turfs from active

pub const MINIMUM_AIR_RATIO_TO_SUSPEND: f32 = 0.1; //Ratio of air that must move to/from a tile to reset group processing
pub const MINIMUM_AIR_RATIO_TO_MOVE: f32 = 0.001; //Minimum ratio of air that must move to/from a tile
pub const MINIMUM_AIR_TO_SUSPEND: f32 = MOLES_CELLSTANDARD * MINIMUM_AIR_RATIO_TO_SUSPEND; //Minimum amount of air that has to move before a group processing can be suspended
pub const MINIMUM_MOLES_DELTA_TO_MOVE: f32 = MOLES_CELLSTANDARD * MINIMUM_AIR_RATIO_TO_MOVE; //Either this must be active
pub const MINIMUM_TEMPERATURE_TO_MOVE: f32 = T20C + 100.0; //or this (or both, obviously)
pub const MINIMUM_TEMPERATURE_DELTA_TO_SUSPEND: f32 = 4.0; //Minimum temperature difference before group processing is suspended
pub const MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER: f32 = 0.5; //Minimum temperature difference before the gas temperatures are just set to be equal
pub const MINIMUM_TEMPERATURE_FOR_SUPERCONDUCTION: f32 = T20C + 10.0;
pub const MINIMUM_TEMPERATURE_START_SUPERCONDUCTION: f32 = T20C + 200.0;

//HEAT TRANSFER COEFFICIENTS
//Must be between 0 and 1. Values closer to 1 equalize temperature faster
//Should not exceed 0.4 else strange heat flow occur
pub const WALL_HEAT_TRANSFER_COEFFICIENT: f32 = 0.0;
pub const OPEN_HEAT_TRANSFER_COEFFICIENT: f32 = 0.4;
pub const WINDOW_HEAT_TRANSFER_COEFFICIENT: f32 = 0.1; //a hack for now
pub const HEAT_CAPACITY_VACUUM: f32 = 7000.0; //a hack to help make vacuums "cold", sacrificing realism for gameplay

//FIRE
pub const FIRE_MINIMUM_TEMPERATURE_TO_SPREAD: f32 = 150.0 + T0C;
pub const FIRE_MINIMUM_TEMPERATURE_TO_EXIST: f32 = 100.0 + T0C;
pub const FIRE_SPREAD_RADIOSITY_SCALE: f32 = 0.85;
pub const FIRE_GROWTH_RATE: f32 = 40000.0; //For small fires
pub const PLASMA_MINIMUM_BURN_TEMPERATURE: f32 = 100.0 + T0C;
pub const PLASMA_UPPER_TEMPERATURE: f32 = 1370.0 + T0C;
pub const PLASMA_OXYGEN_FULLBURN: f32 = 10.0;

//GASES
pub const MIN_TOXIC_GAS_DAMAGE: i32 = 1;
pub const MAX_TOXIC_GAS_DAMAGE: i32 = 10;
pub const MOLES_GAS_VISIBLE: f32 = 0.25; //Moles in a standard cell after which gases are visible

pub const FACTOR_GAS_VISIBLE_MAX: f32 = 20.0; //moles_visible * FACTOR_GAS_VISIBLE_MAX = Moles after which gas is at maximum visibility
pub const MOLES_GAS_VISIBLE_STEP: f32 = 0.25; //Mole step for alpha updates. This means alpha can update at 0.25, 0.5, 0.75 and so on

//REACTIONS
//return values for reactions (bitflags)
pub const NO_REACTION: i32 = 0;
pub const REACTING: i32 = 1;
pub const STOP_REACTIONS: i32 = 2;
