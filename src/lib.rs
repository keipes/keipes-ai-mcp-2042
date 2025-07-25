//! Battlefield 2042 Weapon Statistics Library
//!
//! This library provides access to weapon statistics and damage calculations
//! for Battlefield 2042, with PostgreSQL backend storage and streaming query support.

pub mod client;
pub mod database;
pub mod error;
pub mod models;

#[cfg(test)]
pub mod test_utils;

// Re-export main types for easier usage
pub use client::StatsClient;
pub use database::DatabaseManager;
pub use error::{Result, StatsError};
pub use models::{
    AmmoType, Barrel, Category, ConfigDropoff, Configuration, DatabaseConfig, ValidationReport, Weapon, WeaponAmmoStats,
};
