use crate::intervals_client::Activity;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Serialize, Deserialize)]
pub struct ActivityIndex {
    pub athlete_id: String,
    pub last_updated: String,
    pub geojson_activities: HashSet<String>,
    pub empty_activities: HashSet<String>,
}

impl ActivityIndex {
    pub fn insert_geojson(&mut self, activity_id: &str, activity_hash: &str) {
        let key = Self::create_key(activity_id, activity_hash);
        self.geojson_activities.insert(key);
    }

    pub fn insert_empty(&mut self, activity_id: &str, activity_hash: &str) {
        let key = Self::create_key(activity_id, activity_hash);
        self.empty_activities.insert(key);
    }

    pub fn total_activities(&self) -> usize {
        self.geojson_activities.len() + self.empty_activities.len()
    }

    pub fn new_empty(athlete_id: String) -> Self {
        Self {
            athlete_id,
            last_updated: chrono::Utc::now().to_rfc3339(),
            geojson_activities: HashSet::new(),
            empty_activities: HashSet::new(),
        }
    }

    pub fn try_copy(&self, activity: &Activity, target: &mut ActivityIndex) -> bool {
        let activity_hash = activity.compute_hash();
        let key = Self::create_key(&activity.id, &activity_hash);
        
        if self.geojson_activities.contains(&key) {
            target.geojson_activities.insert(key);
            true
        } else if self.empty_activities.contains(&key) {
            target.empty_activities.insert(key);
            true
        } else {
            false
        }
    }

    pub fn create_key(activity_id: &str, activity_hash: &str) -> String {
        format!("{activity_id}:{activity_hash}")
    }
}