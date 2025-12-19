//! Organization membership projection handler.
//!
//! Handles org_member.* events, updating the org_members_view table.

use async_trait::async_trait;
use plfm_events::{event_types, MemberRole, OrgMemberAddedPayload, OrgMemberRemovedPayload};
use serde::Deserialize;
use tracing::{debug, instrument};

use crate::db::EventRow;

use super::{ProjectionError, ProjectionHandler, ProjectionResult};

/// Projection handler for org membership.
pub struct MembersProjection;

#[derive(Debug, Deserialize)]
struct OrgMemberRoleUpdatedPayload {
    old_role: MemberRole,
    new_role: MemberRole,
}

fn role_label(role: MemberRole) -> &'static str {
    match role {
        MemberRole::Owner => "owner",
        MemberRole::Admin => "admin",
        MemberRole::Developer => "developer",
        MemberRole::Readonly => "readonly",
    }
}

#[async_trait]
impl ProjectionHandler for MembersProjection {
    fn name(&self) -> &'static str {
        "members"
    }

    fn event_types(&self) -> &'static [&'static str] {
        &[
            event_types::ORG_MEMBER_ADDED,
            event_types::ORG_MEMBER_ROLE_UPDATED,
            event_types::ORG_MEMBER_REMOVED,
        ]
    }

    #[instrument(skip(self, tx, event), fields(event_id = event.event_id, event_type = %event.event_type))]
    async fn apply(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        match event.event_type.as_str() {
            event_types::ORG_MEMBER_ADDED => self.handle_member_added(tx, event).await,
            event_types::ORG_MEMBER_ROLE_UPDATED => self.handle_role_updated(tx, event).await,
            event_types::ORG_MEMBER_REMOVED => self.handle_member_removed(tx, event).await,
            _ => {
                debug!(event_type = %event.event_type, "Ignoring unknown event type");
                Ok(())
            }
        }
    }
}

impl MembersProjection {
    async fn handle_member_added(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: OrgMemberAddedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        debug!(
            member_id = %payload.member_id,
            org_id = %payload.org_id,
            email = %payload.email,
            role = %role_label(payload.role),
            "Upserting member into org_members_view"
        );

        sqlx::query(
            r#"
            INSERT INTO org_members_view (
                member_id,
                org_id,
                email,
                role,
                resource_version,
                created_at,
                updated_at,
                is_deleted
            )
            VALUES ($1, $2, $3, $4, 1, $5, $5, false)
            ON CONFLICT (member_id) DO UPDATE SET
                email = EXCLUDED.email,
                role = EXCLUDED.role,
                is_deleted = false,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(payload.member_id.to_string())
        .bind(payload.org_id.to_string())
        .bind(&payload.email)
        .bind(role_label(payload.role))
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    async fn handle_role_updated(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: OrgMemberRoleUpdatedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        debug!(
            member_id = %event.aggregate_id,
            old_role = %role_label(payload.old_role),
            new_role = %role_label(payload.new_role),
            "Updating member role in org_members_view"
        );

        sqlx::query(
            r#"
            UPDATE org_members_view
            SET role = $2,
                resource_version = resource_version + 1,
                updated_at = $3
            WHERE member_id = $1 AND NOT is_deleted
            "#,
        )
        .bind(&event.aggregate_id)
        .bind(role_label(payload.new_role))
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    async fn handle_member_removed(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: OrgMemberRemovedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        debug!(
            member_id = %payload.member_id,
            org_id = %payload.org_id,
            email = %payload.email,
            "Soft-deleting member in org_members_view"
        );

        sqlx::query(
            r#"
            UPDATE org_members_view
            SET is_deleted = true,
                resource_version = resource_version + 1,
                updated_at = $2
            WHERE member_id = $1
            "#,
        )
        .bind(payload.member_id.to_string())
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_members_projection_name() {
        let proj = MembersProjection;
        assert_eq!(proj.name(), "members");
    }

    #[test]
    fn test_members_projection_event_types() {
        let proj = MembersProjection;
        assert!(proj.event_types().contains(&event_types::ORG_MEMBER_ADDED));
        assert!(proj
            .event_types()
            .contains(&event_types::ORG_MEMBER_ROLE_UPDATED));
        assert!(proj
            .event_types()
            .contains(&event_types::ORG_MEMBER_REMOVED));
    }
}
