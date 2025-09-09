use anyhow::Result;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::AttributeValue;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{error, info, warn};

const TABLE_NAME: &str = "ridelines-sync-status";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncStatus {
    Queued,
    InProgress,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

#[derive(Clone)]
pub struct SyncStatusUpdater {
    client: Client,
    user_id: String,
    sync_id: String,
}

impl SyncStatusUpdater {
    pub fn new(client: Client, user_id: String, sync_id: String) -> Self {
        Self {
            client,
            user_id,
            sync_id,
        }
    }

    pub async fn initialize(&self) -> Result<()> {
        // Update existing record to set startedAt and initialize phases
        let updates = vec![
            ("status", AttributeValue::S("in_progress".to_string())),
            ("startedAt", AttributeValue::S(Utc::now().to_rfc3339())),
            (
                "phases",
                AttributeValue::M({
                    let mut phases = HashMap::new();
                    phases.insert(
                        "analyzing".to_string(),
                        AttributeValue::M({
                            let mut phase = HashMap::new();
                            phase.insert(
                                "status".to_string(),
                                AttributeValue::S("pending".to_string()),
                            );
                            phase
                        }),
                    );
                    phases.insert(
                        "downloading".to_string(),
                        AttributeValue::M({
                            let mut phase = HashMap::new();
                            phase.insert(
                                "status".to_string(),
                                AttributeValue::S("pending".to_string()),
                            );
                            phase
                        }),
                    );
                    phases.insert(
                        "generating".to_string(),
                        AttributeValue::M({
                            let mut phase = HashMap::new();
                            phase.insert(
                                "status".to_string(),
                                AttributeValue::S("pending".to_string()),
                            );
                            phase
                        }),
                    );
                    phases
                }),
            ),
        ];

        self.batch_update(updates).await?;

        info!(
            "Initialized sync status for user {} sync {}",
            self.user_id, self.sync_id
        );
        Ok(())
    }

    pub fn start_analyzing(&self) {
        let updater = self.clone();
        tokio::spawn(async move {
            if let Err(e) = updater
                .update_phase_status("analyzing", PhaseStatus::InProgress)
                .await
            {
                warn!("Failed to update analyzing phase status: {}", e);
            }
            if let Err(e) = updater
                .update_attribute("phases.analyzing.message", "Loading activity data...")
                .await
            {
                warn!("Failed to update analyzing message: {}", e);
            }
        });
    }

    pub fn complete_analyzing(&self, total: usize, unchanged: usize, changed: usize) {
        let updater = self.clone();
        tokio::spawn(async move {
            let updates = vec![
                (
                    "phases.analyzing.status",
                    AttributeValue::S("completed".to_string()),
                ),
                (
                    "phases.analyzing.totalActivities",
                    AttributeValue::N(total.to_string()),
                ),
                (
                    "phases.analyzing.unchangedActivities",
                    AttributeValue::N(unchanged.to_string()),
                ),
                (
                    "phases.analyzing.changedActivities",
                    AttributeValue::N(changed.to_string()),
                ),
            ];

            if let Err(e) = updater.batch_update(updates).await {
                warn!("Failed to update analyzing completion: {}", e);
            }
            info!(
                "Completed analyzing phase: {} total, {} unchanged, {} changed",
                total, unchanged, changed
            );
        });
    }

    pub fn start_downloading(&self, total_to_process: usize) {
        let updater = self.clone();
        tokio::spawn(async move {
            let updates = vec![
                (
                    "phases.downloading.status",
                    AttributeValue::S("in_progress".to_string()),
                ),
                (
                    "phases.downloading.totalToProcess",
                    AttributeValue::N(total_to_process.to_string()),
                ),
            ];

            if let Err(e) = updater.batch_update(updates).await {
                warn!("Failed to update downloading start: {}", e);
            }
        });
    }

    pub fn update_download_progress(&self, processed: usize) {
        let updater = self.clone();
        tokio::spawn(async move {
            if let Err(e) = updater
                .update_attribute("phases.downloading.processed", &processed.to_string())
                .await
            {
                warn!("Failed to update download progress: {}", e);
            }
        });
    }

    pub fn complete_downloading(&self) {
        let updater = self.clone();
        tokio::spawn(async move {
            if let Err(e) = updater
                .update_phase_status("downloading", PhaseStatus::Completed)
                .await
            {
                warn!("Failed to update downloading completion: {}", e);
            }
        });
    }

    pub fn start_generating(&self) {
        let updater = self.clone();
        tokio::spawn(async move {
            if let Err(e) = updater
                .update_phase_status("generating", PhaseStatus::InProgress)
                .await
            {
                warn!("Failed to update generating phase status: {}", e);
            }
            if let Err(e) = updater
                .update_attribute("phases.generating.message", "Generating map tiles...")
                .await
            {
                warn!("Failed to update generating message: {}", e);
            }
        });
    }

    pub fn complete_generating(&self) {
        let updater = self.clone();
        tokio::spawn(async move {
            if let Err(e) = updater
                .update_phase_status("generating", PhaseStatus::Completed)
                .await
            {
                warn!("Failed to update generating completion: {}", e);
            }
        });
    }

    pub fn mark_completed(&self) {
        let updater = self.clone();
        tokio::spawn(async move {
            let updates = vec![
                ("status", AttributeValue::S("completed".to_string())),
                ("completedAt", AttributeValue::S(Utc::now().to_rfc3339())),
            ];

            if let Err(e) = updater.batch_update(updates).await {
                warn!("Failed to mark sync as completed: {}", e);
            }
            info!(
                "Sync completed for user {} sync {}",
                updater.user_id, updater.sync_id
            );
        });
    }

    pub fn mark_failed(&self, error: &str) {
        let updater = self.clone();
        let error = error.to_string();
        tokio::spawn(async move {
            let updates = vec![
                ("status", AttributeValue::S("failed".to_string())),
                ("completedAt", AttributeValue::S(Utc::now().to_rfc3339())),
                ("error", AttributeValue::S(error.clone())),
            ];

            if let Err(e) = updater.batch_update(updates).await {
                warn!("Failed to mark sync as failed: {}", e);
            }
            error!(
                "Sync failed for user {} sync {}: {}",
                updater.user_id, updater.sync_id, error
            );
        });
    }

    async fn update_phase_status(&self, phase: &str, status: PhaseStatus) -> Result<()> {
        let status_str = match status {
            PhaseStatus::Pending => "pending",
            PhaseStatus::InProgress => "in_progress",
            PhaseStatus::Completed => "completed",
            PhaseStatus::Failed => "failed",
        };
        self.update_attribute(&format!("phases.{}.status", phase), status_str)
            .await
    }

    async fn update_attribute(&self, path: &str, value: &str) -> Result<()> {
        let update_expression = format!("SET {} = :val", path);

        match self
            .client
            .update_item()
            .table_name(TABLE_NAME)
            .key("userId", AttributeValue::S(self.user_id.clone()))
            .key("syncId", AttributeValue::S(self.sync_id.clone()))
            .update_expression(update_expression)
            .expression_attribute_values(":val", AttributeValue::S(value.to_string()))
            .send()
            .await
        {
            Ok(_) => Ok(()),
            Err(e) => {
                error!(
                    "DynamoDB update_attribute failed for path '{}' with value '{}': {:?}",
                    path, value, e
                );
                Err(anyhow::anyhow!("DynamoDB update failed: {}", e))
            }
        }
    }

    async fn batch_update(&self, updates: Vec<(&str, AttributeValue)>) -> Result<()> {
        let mut update_expression = "SET ".to_string();
        let mut expression_values = HashMap::new();

        for (i, (path, value)) in updates.iter().enumerate() {
            if i > 0 {
                update_expression.push_str(", ");
            }
            let placeholder = format!(":val{}", i);
            update_expression.push_str(&format!("{} = {}", path, placeholder));
            expression_values.insert(placeholder, value.clone());
        }

        let mut request = self
            .client
            .update_item()
            .table_name(TABLE_NAME)
            .key("userId", AttributeValue::S(self.user_id.clone()))
            .key("syncId", AttributeValue::S(self.sync_id.clone()))
            .update_expression(update_expression);

        for (key, value) in expression_values {
            request = request.expression_attribute_values(key, value);
        }

        match request.send().await {
            Ok(_) => Ok(()),
            Err(e) => {
                error!(
                    "DynamoDB update failed for user {} sync {}: {:?}",
                    self.user_id, self.sync_id, e
                );
                Err(anyhow::anyhow!("DynamoDB update failed: {}", e))
            }
        }
    }
}
