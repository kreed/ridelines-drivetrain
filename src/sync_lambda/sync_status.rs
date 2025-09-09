use anyhow::Result;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::AttributeValue;
use chrono::Utc;
use std::collections::HashMap;
use tracing::{error, info};

const TABLE_NAME: &str = "ridelines-sync-status";

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
        // Use update to preserve existing fields like requestedAt
        let mut update = UpdateBuilder::new();
        update
            .set("status", "in_progress")
            .set("startedAt", &Utc::now().to_rfc3339())
            .set_value(
                "phases",
                AttributeValue::M(
                    [
                        (
                            "analyzing".to_string(),
                            AttributeValue::M(
                                [(
                                    "status".to_string(),
                                    AttributeValue::S("pending".to_string()),
                                )]
                                .into_iter()
                                .collect(),
                            ),
                        ),
                        (
                            "downloading".to_string(),
                            AttributeValue::M(
                                [(
                                    "status".to_string(),
                                    AttributeValue::S("pending".to_string()),
                                )]
                                .into_iter()
                                .collect(),
                            ),
                        ),
                        (
                            "generating".to_string(),
                            AttributeValue::M(
                                [(
                                    "status".to_string(),
                                    AttributeValue::S("pending".to_string()),
                                )]
                                .into_iter()
                                .collect(),
                            ),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                ),
            );

        self.execute_update(update).await?;
        info!(
            "Initialized sync status for user {} sync {}",
            self.user_id, self.sync_id
        );
        Ok(())
    }

    pub fn start_analyzing(&self) {
        self.spawn_update(|u| {
            u.set("phases.analyzing.status", "in_progress")
                .set("phases.analyzing.message", "Loading activity data...")
        });
    }

    pub fn complete_analyzing(&self, total: usize, unchanged: usize, changed: usize) {
        self.spawn_update(move |u| {
            u.set("phases.analyzing.status", "completed")
                .set_number("phases.analyzing.totalActivities", total as i64)
                .set_number("phases.analyzing.unchangedActivities", unchanged as i64)
                .set_number("phases.analyzing.changedActivities", changed as i64)
        });
        info!(
            "Completed analyzing: {} total, {} unchanged, {} changed",
            total, unchanged, changed
        );
    }

    pub fn start_downloading(&self, total_to_process: usize) {
        self.spawn_update(move |u| {
            u.set("phases.downloading.status", "in_progress")
                .set_number("phases.downloading.totalToProcess", total_to_process as i64)
        });
    }

    pub fn update_download_progress(&self, processed: usize) {
        self.spawn_update(move |u| u.set_number("phases.downloading.processed", processed as i64));
    }

    pub fn complete_downloading(&self) {
        self.spawn_update(|u| u.set("phases.downloading.status", "completed"));
    }

    pub fn start_generating(&self) {
        self.spawn_update(|u| {
            u.set("phases.generating.status", "in_progress")
                .set("phases.generating.message", "Generating map tiles...")
        });
    }

    pub fn complete_generating(&self) {
        self.spawn_update(|u| u.set("phases.generating.status", "completed"));
    }

    pub fn mark_completed(&self) {
        self.spawn_update(|u| {
            u.set("status", "completed")
                .set("completedAt", &Utc::now().to_rfc3339())
        });
        info!(
            "Sync completed for user {} sync {}",
            self.user_id, self.sync_id
        );
    }

    pub fn mark_failed(&self, error: &str) {
        let error = error.to_string();
        let user_id = self.user_id.clone();
        let sync_id = self.sync_id.clone();

        error!(
            "Sync failed for user {} sync {}: {}",
            user_id, sync_id, error
        );
        self.spawn_update(move |u| {
            u.set("status", "failed")
                .set("completedAt", &Utc::now().to_rfc3339())
                .set("error", &error)
        });
    }

    fn spawn_update<F>(&self, f: F)
    where
        F: FnOnce(&mut UpdateBuilder) -> &mut UpdateBuilder + Send + 'static,
    {
        let updater = self.clone();
        tokio::spawn(async move {
            let mut update = UpdateBuilder::new();
            f(&mut update);
            let _ = updater.execute_update(update).await;
        });
    }

    async fn execute_update(&self, update: UpdateBuilder) -> Result<()> {
        let mut req = self
            .client
            .update_item()
            .table_name(TABLE_NAME)
            .key("userId", AttributeValue::S(self.user_id.clone()))
            .key("syncId", AttributeValue::S(self.sync_id.clone()))
            .update_expression(update.expression);

        for (k, v) in update.values {
            req = req.expression_attribute_values(k, v);
        }

        for (k, v) in update.names {
            req = req.expression_attribute_names(k, v);
        }

        match req.send().await {
            Ok(_) => Ok(()),
            Err(e) => {
                error!("DynamoDB update failed: {:?}", e);
                Err(anyhow::anyhow!("DynamoDB update failed: {}", e))
            }
        }
    }
}

struct UpdateBuilder {
    parts: Vec<String>,
    values: HashMap<String, AttributeValue>,
    names: HashMap<String, String>,
    expression: String,
}

impl UpdateBuilder {
    fn new() -> Self {
        Self {
            parts: Vec::new(),
            values: HashMap::new(),
            names: HashMap::new(),
            expression: String::new(),
        }
    }

    fn set(&mut self, path: &str, value: &str) -> &mut Self {
        self.set_value(path, AttributeValue::S(value.to_string()))
    }

    fn set_number(&mut self, path: &str, value: i64) -> &mut Self {
        self.set_value(path, AttributeValue::N(value.to_string()))
    }

    fn set_value(&mut self, path: &str, value: AttributeValue) -> &mut Self {
        let val_key = format!(":v{}", self.values.len());
        self.values.insert(val_key.clone(), value);

        let safe_path = path
            .split('.')
            .map(|part| match part {
                "status" | "error" => {
                    let placeholder = format!("#{}", part);
                    self.names.insert(placeholder.clone(), part.to_string());
                    placeholder
                }
                _ => part.to_string(),
            })
            .collect::<Vec<_>>()
            .join(".");

        self.parts.push(format!("{} = {}", safe_path, val_key));
        self.expression = format!("SET {}", self.parts.join(", "));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_update_builder_expression() {
        let mut builder = UpdateBuilder::new();
        builder
            .set("status", "in_progress")
            .set("startedAt", "2024-01-01T00:00:00Z")
            .set("phases.analyzing.status", "pending");

        println!("Expression: {}", builder.expression);
        println!("Names: {:?}", builder.names);
        println!("Values: {:?}", builder.values);

        assert!(builder.expression.contains("SET"));
        assert!(builder.expression.contains("#status"));
        assert_eq!(builder.names.len(), 1);
        assert_eq!(builder.names.get("#status"), Some(&"status".to_string()));
    }
}
