use rusqlite::params;
use uuid::Uuid;

use crate::db::DbPool;
use crate::models::AlertRule;

/// Evaluate alert rules when a status update or error is ingested.
/// `condition` is "agent_error" (or "agent_errored") or "agent_offline" (or "agent_silent").
pub async fn evaluate_alerts(
    db: DbPool,
    condition: &str,
    agent_id: Option<&str>,
    agent_name: &str,
    message: &str,
) {
    let (c1, c2) = match condition {
        "agent_errored" | "agent_error" => ("agent_error", "agent_errored"),
        "agent_silent" | "agent_offline" => ("agent_offline", "agent_silent"),
        _ => (condition, condition),
    };

    let rules = {
        let conn = db.get().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, name, condition, agent_id, webhook_url, enabled, silence_minutes, created_at
                 FROM alert_rules
                 WHERE enabled = 1 AND condition IN (?1, ?2)",
            )
            .unwrap();

        let rules: Vec<AlertRule> = stmt
            .query_map(params![c1, c2], |row| {
                Ok(AlertRule {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    condition: row.get(2)?,
                    agent_id: row.get(3)?,
                    webhook_url: row.get(4)?,
                    enabled: row.get::<_, i64>(5)? != 0,
                    silence_minutes: row.get(6)?,
                    created_at: row.get(7)?,
                })
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        rules
    };

    for rule in rules {
        // Check if rule applies to this agent
        if let Some(ref rule_agent_id) = rule.agent_id {
            if !rule_agent_id.is_empty() {
                if let Some(agent_id) = agent_id {
                    if rule_agent_id != agent_id {
                        continue;
                    }
                }
            }
        }

        // Check silence window — don't re-fire if we already fired recently
        let should_fire = {
            let conn = db.get().unwrap();
            let recent_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM alert_history
                     WHERE alert_rule_id = ?1
                     AND created_at > datetime('now', ?2)",
                    params![rule.id, format!("-{} minutes", rule.silence_minutes)],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            recent_count == 0
        };

        if !should_fire {
            tracing::debug!(
                "Alert rule '{}' silenced (fired within {} minutes)",
                rule.name,
                rule.silence_minutes
            );
            continue;
        }

        // Fire the alert
        let alert_msg = format!("[{}] Agent '{}': {}", condition, agent_name, message);

        let history_id = Uuid::new_v4().to_string();
        let delivery_status =
            send_discord_webhook(&rule.webhook_url, agent_name, condition, message).await;

        // Record in alert history
        {
            let conn = db.get().unwrap();
            let _ = conn.execute(
                "INSERT INTO alert_history (id, alert_rule_id, agent_id, agent_name, condition, message, delivery_status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    history_id,
                    rule.id,
                    agent_id,
                    agent_name,
                    condition,
                    alert_msg,
                    delivery_status,
                ],
            );
        }

        tracing::info!(
            "Alert '{}' fired for agent '{}': {}",
            rule.name,
            agent_name,
            delivery_status
        );
    }
}

/// Send a Discord webhook notification.
async fn send_discord_webhook(
    webhook_url: &str,
    agent_name: &str,
    condition: &str,
    message: &str,
) -> String {
    let condition_label = match condition {
        "agent_errored" => "Agent Error",
        "agent_silent" => "Agent Silent / Offline",
        _ => condition,
    };

    let color = match condition {
        "agent_errored" => 0xdc2626, // red
        "agent_silent" => 0x6b7280,  // gray
        _ => 0xca8a04,               // yellow
    };

    let now = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S UTC")
        .to_string();

    let payload = serde_json::json!({
        "embeds": [{
            "title": format!("MCPolly Alert: {}", condition_label),
            "description": message,
            "color": color,
            "fields": [
                {
                    "name": "Agent",
                    "value": agent_name,
                    "inline": true
                },
                {
                    "name": "Condition",
                    "value": condition_label,
                    "inline": true
                },
                {
                    "name": "Timestamp",
                    "value": now,
                    "inline": true
                }
            ]
        }]
    });

    let client = reqwest::Client::new();
    match client.post(webhook_url).json(&payload).send().await {
        Ok(resp) if resp.status().is_success() => "sent".to_string(),
        Ok(resp) => {
            let status = resp.status();
            tracing::warn!("Discord webhook returned {}", status);

            // Retry once after a short delay
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            match client.post(webhook_url).json(&payload).send().await {
                Ok(r) if r.status().is_success() => "sent_retry".to_string(),
                _ => format!("failed_{}", status),
            }
        }
        Err(e) => {
            tracing::error!("Discord webhook error: {}", e);

            // Retry once
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            match client.post(webhook_url).json(&payload).send().await {
                Ok(r) if r.status().is_success() => "sent_retry".to_string(),
                _ => format!("failed_{}", e),
            }
        }
    }
}

/// Background task that checks for silent agents every 60 seconds.
pub async fn silent_agent_checker(db: DbPool) {
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));

    loop {
        interval.tick().await;

        // Check for expired stop requests
        {
            let timeout_secs: i64 = std::env::var("STOP_TIMEOUT_SECONDS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(300);

            let conn = db.get().unwrap();
            let mut stmt = conn
                .prepare(
                    "SELECT sr.id, sr.agent_id, a.name FROM stop_requests sr
                     JOIN agents a ON sr.agent_id = a.id
                     WHERE sr.status = 'pending'
                     AND sr.created_at < datetime('now', ?1)",
                )
                .unwrap();

            let timeout_modifier = format!("-{} seconds", timeout_secs);
            let expired: Vec<(String, String, String)> = stmt
                .query_map(params![timeout_modifier], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .unwrap()
                .filter_map(|r| r.ok())
                .collect();

            for (stop_id, agent_id, agent_name) in &expired {
                conn.execute(
                    "UPDATE stop_requests SET status = 'expired', resolved_at = datetime('now') WHERE id = ?1",
                    params![stop_id],
                ).ok();

                let status_id = uuid::Uuid::new_v4().to_string();
                conn.execute(
                    "INSERT INTO status_updates (id, agent_id, state, message) VALUES (?1, ?2, 'stopped', ?3)",
                    params![status_id, agent_id, "Stop request expired — agent did not acknowledge within timeout"],
                ).ok();

                conn.execute(
                    "UPDATE agents SET current_state = 'stopped', last_message = 'Stop request expired', last_update_at = datetime('now') WHERE id = ?1",
                    params![agent_id],
                ).ok();

                tracing::warn!(
                    "Stop request expired for agent '{}' ({})",
                    agent_name,
                    agent_id
                );
            }
        }

        let rules_and_agents = {
            let conn = db.get().unwrap();

            // Get all enabled agent_silent rules
            let mut stmt = conn
                .prepare(
                    "SELECT id, name, condition, agent_id, webhook_url, enabled, silence_minutes, created_at
                     FROM alert_rules
                     WHERE enabled = 1 AND condition = 'agent_silent'",
                )
                .unwrap();

            let rules: Vec<AlertRule> = stmt
                .query_map([], |row| {
                    Ok(AlertRule {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        condition: row.get(2)?,
                        agent_id: row.get(3)?,
                        webhook_url: row.get(4)?,
                        enabled: row.get::<_, i64>(5)? != 0,
                        silence_minutes: row.get(6)?,
                        created_at: row.get(7)?,
                    })
                })
                .unwrap()
                .filter_map(|r| r.ok())
                .collect();

            // Get all agents with their last update times
            let mut agents_stmt = conn
                .prepare("SELECT id, name, last_update_at FROM agents")
                .unwrap();

            let agents: Vec<(String, String, Option<String>)> = agents_stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect();

            (rules, agents)
        };

        let (rules, agents) = rules_and_agents;

        for rule in &rules {
            for (agent_id, agent_name, last_update) in &agents {
                // Check if rule applies to this agent
                if let Some(ref rule_agent_id) = rule.agent_id {
                    if !rule_agent_id.is_empty() && rule_agent_id != agent_id {
                        continue;
                    }
                }

                // Check if agent is silent
                let is_silent = match last_update {
                    Some(last) => {
                        let conn = db.get().unwrap();
                        let is_old: bool = conn
                            .query_row(
                                "SELECT ?1 < datetime('now', ?2)",
                                params![last, format!("-{} minutes", rule.silence_minutes)],
                                |row| row.get(0),
                            )
                            .unwrap_or(false);
                        is_old
                    }
                    None => true, // Never updated = silent
                };

                if is_silent {
                    let msg = format!(
                        "Agent '{}' has been silent for over {} minutes",
                        agent_name, rule.silence_minutes
                    );
                    evaluate_alerts(db.clone(), "agent_silent", Some(agent_id), agent_name, &msg)
                        .await;
                }
            }
        }
    }
}
