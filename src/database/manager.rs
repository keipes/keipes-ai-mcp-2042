//! Database manager for schema and data operations

use crate::models::{DatabaseConfig, ValidationReport};
use crate::{Result, StatsError};
use sqlx::PgPool;
use tracing::{debug, info};

/// Manages database connections and operations
pub struct DatabaseManager {
    pool: PgPool,
}

impl DatabaseManager {
    /// Create a new database manager with the given configuration
    pub async fn new(config: &DatabaseConfig) -> Result<Self> {
        info!("Connecting to database: {}", config.url());

        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(config.max_connections)
            .connect(config.url())
            .await?;

        debug!("Database connection established with max_connections: {}", config.max_connections);

        Ok(Self { pool })
    }

    /// Get a reference to the connection pool
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Test the database connection
    pub async fn test_connection(&self) -> Result<()> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;

        info!("Database connection test successful");
        Ok(())
    }

    /// Create the database schema
    pub async fn create_schema(&self) -> Result<()> {
        info!("Creating database schema");

        // Execute schema statements in order
        let schema_statements = [
            // Categories table
            r#"
            CREATE TABLE IF NOT EXISTS categories (
                category_id SERIAL PRIMARY KEY,
                category_name VARCHAR(50) NOT NULL UNIQUE
            )
            "#,
            
            // Weapons table
            r#"
            CREATE TABLE IF NOT EXISTS weapons (
                weapon_id SERIAL PRIMARY KEY,
                weapon_name VARCHAR(100) NOT NULL UNIQUE,
                category_id INTEGER NOT NULL REFERENCES categories(category_id)
            )
            "#,
            
            // Barrels table
            r#"
            CREATE TABLE IF NOT EXISTS barrels (
                barrel_id SERIAL PRIMARY KEY,
                barrel_name VARCHAR(100) NOT NULL UNIQUE
            )
            "#,
            
            // Ammo types table
            r#"
            CREATE TABLE IF NOT EXISTS ammo_types (
                ammo_id SERIAL PRIMARY KEY,
                ammo_type_name VARCHAR(100) NOT NULL UNIQUE
            )
            "#,
            
            // Weapon ammo compatibility and stats
            r#"
            CREATE TABLE IF NOT EXISTS weapon_ammo_stats (
                weapon_id INTEGER NOT NULL REFERENCES weapons(weapon_id),
                ammo_id INTEGER NOT NULL REFERENCES ammo_types(ammo_id),
                magazine_size SMALLINT NOT NULL,
                empty_reload_time DECIMAL(4,2),
                tactical_reload_time DECIMAL(4,2),
                headshot_multiplier DECIMAL(3,1) NOT NULL,
                pellet_count SMALLINT DEFAULT 1,
                PRIMARY KEY (weapon_id, ammo_id)
            )
            "#,
            
            // Configurations table
            r#"
            CREATE TABLE IF NOT EXISTS configurations (
                config_id SERIAL PRIMARY KEY,
                weapon_id INTEGER NOT NULL REFERENCES weapons(weapon_id),
                barrel_id INTEGER NOT NULL REFERENCES barrels(barrel_id),
                ammo_id INTEGER NOT NULL REFERENCES ammo_types(ammo_id),
                velocity SMALLINT NOT NULL,
                rpm_single SMALLINT,
                rpm_burst SMALLINT,
                rpm_auto SMALLINT,
                UNIQUE(weapon_id, barrel_id, ammo_id)
            )
            "#,
            
            // Damage dropoff data
            r#"
            CREATE TABLE IF NOT EXISTS config_dropoffs (
                config_id INTEGER NOT NULL REFERENCES configurations(config_id),
                range SMALLINT NOT NULL,
                damage DECIMAL(5,1) NOT NULL,
                PRIMARY KEY (config_id, range)
            )
            "#,
        ];

        // Execute schema creation statements
        for statement in &schema_statements {
            sqlx::query(statement).execute(&self.pool).await?;
        }

        // Create indexes
        let index_statements = [
            "CREATE INDEX IF NOT EXISTS idx_weapons_category ON weapons(category_id)",
            "CREATE INDEX IF NOT EXISTS idx_configurations_weapon ON configurations(weapon_id)",
            "CREATE INDEX IF NOT EXISTS idx_config_dropoffs_config ON config_dropoffs(config_id)",
            "CREATE INDEX IF NOT EXISTS idx_config_dropoffs_range ON config_dropoffs(range)",
            "CREATE INDEX IF NOT EXISTS idx_weapon_ammo_stats_weapon ON weapon_ammo_stats(weapon_id)",
        ];

        for statement in &index_statements {
            sqlx::query(statement).execute(&self.pool).await?;
        }

        info!("Database schema created successfully");
        Ok(())
    }

    /// Populate database from embedded weapons data
    pub async fn populate_from_embedded_data(&self) -> Result<()> {
        info!("Populating database from embedded weapons data");
        
        // Use embedded weapons.json data
        const WEAPONS_JSON: &str = include_str!("../../weapons.json");
        
        self.populate_from_json_str(WEAPONS_JSON).await
    }

    /// Internal method to populate from JSON string
    async fn populate_from_json_str(&self, json_content: &str) -> Result<()> {
        use crate::models::WeaponsData;
        use std::collections::{HashMap, HashSet};

        let weapons_data: WeaponsData =
            serde_json::from_str(json_content).map_err(|e| StatsError::ParseError(e))?;

        debug!(
            "Parsed {} categories from JSON",
            weapons_data.categories.len()
        );

        // Extract unique values
        let mut categories = Vec::new();
        let mut barrels = HashSet::new();
        let mut ammo_types = HashSet::new();
        let mut weapons = Vec::new();
        let mut weapon_ammo_stats = Vec::new();

        // Process categories and weapons
        for (category_idx, category) in weapons_data.categories.iter().enumerate() {
            categories.push((category_idx as i32 + 1, category.name.clone()));

            for weapon in &category.weapons {
                let weapon_id = weapons.len() as i32 + 1;
                weapons.push((weapon_id, weapon.name.clone(), category_idx as i32 + 1));

                // Collect barrel types and ammo types from stats
                for stat in &weapon.stats {
                    barrels.insert(stat.barrel_type.clone());
                    ammo_types.insert(stat.ammo_type.clone());
                }

                // Process ammo stats
                for (ammo_name, ammo_stat) in &weapon.ammo_stats {
                    ammo_types.insert(ammo_name.clone());
                    weapon_ammo_stats.push((
                        weapon_id,
                        ammo_name.clone(),
                        ammo_stat.mag_size,
                        ammo_stat.empty_reload,
                        ammo_stat.tactical_reload,
                        ammo_stat.headshot_multiplier,
                        ammo_stat.pellet_count,
                    ));
                }
            }
        }

        // Convert sets to sorted vectors for consistent insertion
        let mut barrels: Vec<String> = barrels.into_iter().collect();
        barrels.sort();
        let mut ammo_types_vec: Vec<String> = ammo_types.into_iter().collect();
        ammo_types_vec.sort();

        // Start transaction
        let mut tx = self.pool.begin().await?;

        // Insert categories
        for (category_id, category_name) in categories {
            sqlx::query(
                "INSERT INTO categories (category_id, category_name) VALUES ($1, $2) ON CONFLICT (category_name) DO NOTHING"
            )
            .bind(category_id)
            .bind(&category_name)
            .execute(&mut *tx)
            .await?;
        }

        // Insert barrels
        for (idx, barrel_name) in barrels.iter().enumerate() {
            sqlx::query(
                "INSERT INTO barrels (barrel_id, barrel_name) VALUES ($1, $2) ON CONFLICT (barrel_name) DO NOTHING"
            )
            .bind(idx as i32 + 1)
            .bind(barrel_name)
            .execute(&mut *tx)
            .await?;
        }

        // Insert ammo types
        for (idx, ammo_name) in ammo_types_vec.iter().enumerate() {
            sqlx::query(
                "INSERT INTO ammo_types (ammo_id, ammo_type_name) VALUES ($1, $2) ON CONFLICT (ammo_type_name) DO NOTHING"
            )
            .bind(idx as i32 + 1)
            .bind(ammo_name)
            .execute(&mut *tx)
            .await?;
        }

        // Insert weapons
        for (weapon_id, weapon_name, category_id) in weapons {
            sqlx::query(
                "INSERT INTO weapons (weapon_id, weapon_name, category_id) VALUES ($1, $2, $3) ON CONFLICT (weapon_name) DO NOTHING"
            )
            .bind(weapon_id)
            .bind(&weapon_name)
            .bind(category_id)
            .execute(&mut *tx)
            .await?;
        }

        // Create lookup maps for IDs
        let barrel_id_map: HashMap<String, i32> = barrels
            .iter()
            .enumerate()
            .map(|(idx, name)| (name.clone(), idx as i32 + 1))
            .collect();

        let ammo_id_map: HashMap<String, i32> = ammo_types_vec
            .iter()
            .enumerate()
            .map(|(idx, name)| (name.clone(), idx as i32 + 1))
            .collect();

        // Insert weapon ammo stats
        for (
            weapon_id,
            ammo_name,
            mag_size,
            empty_reload,
            tactical_reload,
            headshot_mult,
            pellet_count,
        ) in weapon_ammo_stats
        {
            if let Some(&ammo_id) = ammo_id_map.get(&ammo_name) {
                sqlx::query(
                    "INSERT INTO weapon_ammo_stats (weapon_id, ammo_id, magazine_size, empty_reload_time, tactical_reload_time, headshot_multiplier, pellet_count) VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT (weapon_id, ammo_id) DO NOTHING"
                )
                .bind(weapon_id)
                .bind(ammo_id)
                .bind(mag_size)
                .bind(empty_reload)
                .bind(tactical_reload)
                .bind(headshot_mult)
                .bind(pellet_count)
                .execute(&mut *tx)
                .await?;
            }
        }

        // Process configurations and dropoffs
        let mut config_id = 1;
        for category in &weapons_data.categories {
            for weapon in &category.weapons {
                let weapon_id = weapons_data
                    .categories
                    .iter()
                    .take_while(|c| c.name != category.name)
                    .map(|c| c.weapons.len())
                    .sum::<usize>()
                    + category
                        .weapons
                        .iter()
                        .take_while(|w| w.name != weapon.name)
                        .count()
                    + 1;

                for stat in &weapon.stats {
                    if let (Some(&barrel_id), Some(&ammo_id)) = (
                        barrel_id_map.get(&stat.barrel_type),
                        ammo_id_map.get(&stat.ammo_type),
                    ) {
                        // Insert configuration
                        sqlx::query(
                            "INSERT INTO configurations (config_id, weapon_id, barrel_id, ammo_id, velocity, rpm_single, rpm_burst, rpm_auto) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) ON CONFLICT (weapon_id, barrel_id, ammo_id) DO NOTHING"
                        )
                        .bind(config_id)
                        .bind(weapon_id as i32)
                        .bind(barrel_id)
                        .bind(ammo_id)
                        .bind(stat.velocity)
                        .bind(stat.rpm_single)
                        .bind(stat.rpm_burst)
                        .bind(stat.rpm_auto)
                        .execute(&mut *tx)
                        .await?;

                        // Insert dropoffs for this configuration
                        for dropoff in &stat.dropoffs {
                            sqlx::query(
                                "INSERT INTO config_dropoffs (config_id, range, damage) VALUES ($1, $2, $3) ON CONFLICT (config_id, range) DO NOTHING"
                            )
                            .bind(config_id)
                            .bind(dropoff.range)
                            .bind(dropoff.damage)
                            .execute(&mut *tx)
                            .await?;
                        }

                        config_id += 1;
                    }
                }
            }
        }

        // Commit transaction
        tx.commit().await?;

        info!("Database populated successfully from JSON");
        Ok(())
    }

    /// Reset database - Drop and recreate all tables
    pub async fn reset_database(&self) -> Result<()> {
        info!("Resetting database (drop and recreate schema)");

        // Drop all tables and sequences in correct order (reverse dependency order)
        let drop_statements = [
            "DROP TABLE IF EXISTS config_dropoffs CASCADE",
            "DROP TABLE IF EXISTS configurations CASCADE", 
            "DROP TABLE IF EXISTS weapon_ammo_stats CASCADE",
            "DROP TABLE IF EXISTS weapons CASCADE",
            "DROP TABLE IF EXISTS ammo_types CASCADE",
            "DROP TABLE IF EXISTS barrels CASCADE",
            "DROP TABLE IF EXISTS categories CASCADE",
            // Drop sequences explicitly to avoid conflicts
            "DROP SEQUENCE IF EXISTS categories_category_id_seq CASCADE",
            "DROP SEQUENCE IF EXISTS weapons_weapon_id_seq CASCADE",
            "DROP SEQUENCE IF EXISTS barrels_barrel_id_seq CASCADE",
            "DROP SEQUENCE IF EXISTS ammo_types_ammo_id_seq CASCADE",
            "DROP SEQUENCE IF EXISTS configurations_config_id_seq CASCADE",
        ];

        for statement in &drop_statements {
            sqlx::query(statement)
                .execute(&self.pool)
                .await?;
        }

        info!("All tables and sequences dropped successfully");

        // Recreate schema
        self.create_schema().await?;

        info!("Database reset completed successfully");
        Ok(())
    }

    /// Clear all data while keeping schema intact
    pub async fn clear_data(&self) -> Result<()> {
        info!("Clearing all data from database");

        // Start transaction
        let mut tx = self.pool.begin().await?;

        // Delete data in correct order (reverse dependency order)
        sqlx::query("DELETE FROM config_dropoffs")
            .execute(&mut *tx)
            .await?;

        sqlx::query("DELETE FROM configurations")
            .execute(&mut *tx)
            .await?;

        sqlx::query("DELETE FROM weapon_ammo_stats")
            .execute(&mut *tx)
            .await?;

        sqlx::query("DELETE FROM weapons")
            .execute(&mut *tx)
            .await?;

        sqlx::query("DELETE FROM ammo_types")
            .execute(&mut *tx)
            .await?;

        sqlx::query("DELETE FROM barrels")
            .execute(&mut *tx)
            .await?;

        sqlx::query("DELETE FROM categories")
            .execute(&mut *tx)
            .await?;

        // Commit transaction
        tx.commit().await?;

        info!("All data cleared successfully");
        Ok(())
    }

    /// Validate data integrity
    pub async fn validate_data(&self) -> Result<ValidationReport> {
        info!("Validating database data integrity");

        let mut report = ValidationReport {
            is_valid: true,
            issues: Vec::new(),
            table_counts: std::collections::HashMap::new(),
        };

        // Check table counts
        let tables = [
            "categories", "weapons", "barrels", "ammo_types", 
            "weapon_ammo_stats", "configurations", "config_dropoffs"
        ];

        for table in &tables {
            let count: (i64,) = sqlx::query_as(&format!("SELECT COUNT(*) FROM {}", table))
                .fetch_one(&self.pool)
                .await?;
            
            report.table_counts.insert(table.to_string(), count.0);
            
            if count.0 == 0 {
                report.is_valid = false;
                report.issues.push(format!("Table '{}' is empty", table));
            }
        }

        // Simplified referential integrity checks
        let integrity_checks = [
            ("SELECT COUNT(*) FROM weapons w WHERE NOT EXISTS (SELECT 1 FROM categories c WHERE c.category_id = w.category_id)", "weapons reference non-existent categories"),
            ("SELECT COUNT(*) FROM configurations c WHERE NOT EXISTS (SELECT 1 FROM weapons w WHERE w.weapon_id = c.weapon_id) OR NOT EXISTS (SELECT 1 FROM barrels b WHERE b.barrel_id = c.barrel_id) OR NOT EXISTS (SELECT 1 FROM ammo_types a WHERE a.ammo_id = c.ammo_id)", "configurations have invalid references"),
            ("SELECT COUNT(*) FROM config_dropoffs cd WHERE NOT EXISTS (SELECT 1 FROM configurations c WHERE c.config_id = cd.config_id)", "dropoffs reference non-existent configurations"),
            ("SELECT COUNT(*) FROM weapon_ammo_stats was WHERE NOT EXISTS (SELECT 1 FROM weapons w WHERE w.weapon_id = was.weapon_id) OR NOT EXISTS (SELECT 1 FROM ammo_types a WHERE a.ammo_id = was.ammo_id)", "ammo stats have invalid references"),
        ];

        for (query, description) in integrity_checks {
            let count: (i64,) = sqlx::query_as(query).fetch_one(&self.pool).await?;
            if count.0 > 0 {
                report.is_valid = false;
                report.issues.push(format!("{} {}", count.0, description));
            }
        }

        if report.is_valid {
            info!("Database validation passed - all integrity checks successful");
        } else {
            info!("Database validation failed - {} issues found", report.issues.len());
        }

        Ok(report)
    }
}
