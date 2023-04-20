use tokio::task::JoinSet;
use digital_asset_types::dao::asset_data;

use log::{info};

use nft_ingester::{
    tasks::{BgTask, DownloadMetadata, IntoTaskData, DownloadMetadataTask, TaskManager},
    config::{init_logger, setup_config},
    database::setup_database,
    metrics::setup_metrics,
    config::rand_string,
    error::IngesterError,
};

use std::{
    path::PathBuf,
    time
};

use sea_orm::{
    entity::*, query::*, EntityTrait, JsonValue, SqlxPostgresConnector
};

use clap::{arg, command, value_parser};

use sqlx::types::chrono::Utc;

#[tokio::main(flavor = "multi_thread")]
pub async fn main() {
    init_logger();
    info!("Starting bgtask creator");

    let matches = command!() 
        .arg(
            arg!(
                -c --config <FILE> "Sets a custom config file"
            )
            // We don't have syntax yet for optional options, so manually calling `required`
            .required(false)
            .value_parser(value_parser!(PathBuf)),
        )
        .get_matches();

    let config_path = matches.get_one::<PathBuf>("config");
    if let Some(config_path) = config_path {
        println!("Loading config from: {}", config_path.display());
    }

    // Pull Env variables into config struct
    let config = setup_config(config_path);

    // Optionally setup metrics if config demands it
    setup_metrics(&config);

    // One pool many clones, this thing is thread safe and send sync
    let database_pool = setup_database(config.clone()).await;

    // Set up a task pool
    let mut tasks = JoinSet::new();

    //Setup definitions for background tasks
    let task_runner_config = config.background_task_runner_config.clone().unwrap_or_default();
    let bg_task_definitions: Vec<Box<dyn BgTask>> = vec![Box::new(DownloadMetadataTask {
        lock_duration: task_runner_config.lock_duration,
        max_attempts: task_runner_config.max_attempts,
        timeout: Some(time::Duration::from_secs(task_runner_config.timeout.unwrap_or(3))),
    })];

    let mut background_task_manager =
        TaskManager::new(rand_string(), database_pool.clone(), bg_task_definitions);
        
    // This is how we send new bg tasks
    let bg_task_listener = background_task_manager.start_listener(false);
    tasks.spawn(bg_task_listener);

    let bg_task_sender = background_task_manager.get_sender().unwrap();

    // Create new postgres connection
    let conn = SqlxPostgresConnector::from_sqlx_postgres_pool(database_pool.clone());

    // Delete all existing tasks
    let deleted_tasks = tasks::Entity::delete_many()
            .exec(conn)
            .await
            .map_err(|e| e.into());
    
    match deleted_tasks {
        Ok(_) => {
            info!("Deleted a number of tasks {}", deleted_tasks.rows_affected);
        }
        Err(e) => {
            info!("Error deleting tasks: {}", e);
        }
    }

    // Find all the assets with missing metadata
    let asset_data_missing: Result<Vec<asset_data::Model>, IngesterError>  = asset_data::Entity::find()
        .filter(
            Condition::all()
                .add(asset_data::Column::Metadata.eq(JsonValue::String("processing".to_string())))
        )
        .order_by(asset_data::Column::Id, Order::Asc)
        .limit(10000)
        .all(&conn)
        .await
        .map_err(|e| e.into());

    match asset_data_missing {
        Ok(assets) => {
            info!("Found {} assets", assets.len());
            for asset in assets {
                let mut task = DownloadMetadata {
                    asset_data_id: asset.id,
                    uri: asset.metadata_url,
                    created_at: Some(Utc::now().naive_utc()),
                };

                info!("Print task {}", task);
                task.sanitize();
                let task_data = task.into_task_data().unwrap();
                bg_task_sender.send(task_data);
            }
        }
        Err(e) => {
            info!("Error: {}", e);
        }
    }
}
