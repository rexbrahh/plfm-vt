use plfm_id::OrgId;
use serde::Serialize;
use sqlx::PgPool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuotaDimension {
    MaxInstances,
    MaxTotalMemoryBytes,
    MaxEnvs,
    MaxApps,
    MaxRoutes,
    MaxIpv4Allocations,
    MaxVolumes,
    MaxTotalVolumeBytes,
    MaxVolumeAttachments,
}

impl QuotaDimension {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MaxInstances => "max_instances",
            Self::MaxTotalMemoryBytes => "max_total_memory_bytes",
            Self::MaxEnvs => "max_envs",
            Self::MaxApps => "max_apps",
            Self::MaxRoutes => "max_routes",
            Self::MaxIpv4Allocations => "max_ipv4_allocations",
            Self::MaxVolumes => "max_volumes",
            Self::MaxTotalVolumeBytes => "max_total_volume_bytes",
            Self::MaxVolumeAttachments => "max_volume_attachments",
        }
    }

    pub fn default_limit(&self) -> i64 {
        match self {
            Self::MaxInstances => 50,
            Self::MaxTotalMemoryBytes => 64 * 1024 * 1024 * 1024,
            Self::MaxEnvs => 100,
            Self::MaxApps => 20,
            Self::MaxRoutes => 50,
            Self::MaxIpv4Allocations => 5,
            Self::MaxVolumes => 20,
            Self::MaxTotalVolumeBytes => 500 * 1024 * 1024 * 1024,
            Self::MaxVolumeAttachments => 50,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct QuotaExceeded {
    pub dimension: String,
    pub limit: i64,
    pub current_usage: i64,
    pub requested_delta: i64,
}

pub async fn get_effective_limit(
    pool: &PgPool,
    org_id: &OrgId,
    dimension: QuotaDimension,
) -> Result<i64, sqlx::Error> {
    let override_limit: Option<i64> = sqlx::query_scalar(
        "SELECT limit_value FROM org_quotas WHERE org_id = $1 AND dimension = $2",
    )
    .bind(org_id.to_string())
    .bind(dimension.as_str())
    .fetch_optional(pool)
    .await?;

    Ok(override_limit.unwrap_or_else(|| dimension.default_limit()))
}

pub async fn get_current_usage(
    pool: &PgPool,
    org_id: &OrgId,
    dimension: QuotaDimension,
) -> Result<i64, sqlx::Error> {
    let query = match dimension {
        QuotaDimension::MaxInstances => {
            "SELECT COUNT(*)::BIGINT FROM instances_desired_view 
             WHERE org_id = $1 AND desired_state != 'stopped'"
        }
        QuotaDimension::MaxTotalMemoryBytes => {
            "SELECT COALESCE(SUM(memory_limit_bytes), 0)::BIGINT FROM instances_desired_view 
             WHERE org_id = $1 AND desired_state != 'stopped'"
        }
        QuotaDimension::MaxEnvs => {
            "SELECT COUNT(*)::BIGINT FROM envs_view WHERE org_id = $1 AND NOT is_deleted"
        }
        QuotaDimension::MaxApps => {
            "SELECT COUNT(*)::BIGINT FROM apps_view WHERE org_id = $1 AND NOT is_deleted"
        }
        QuotaDimension::MaxRoutes => {
            "SELECT COUNT(*)::BIGINT FROM routes_view WHERE org_id = $1 AND NOT is_deleted"
        }
        QuotaDimension::MaxIpv4Allocations => {
            "SELECT COUNT(*)::BIGINT FROM ipam_ipv4_allocations 
             WHERE org_id = $1 AND released_at IS NULL"
        }
        QuotaDimension::MaxVolumes => {
            "SELECT COUNT(*)::BIGINT FROM volumes_view WHERE org_id = $1 AND NOT is_deleted"
        }
        QuotaDimension::MaxTotalVolumeBytes => {
            "SELECT COALESCE(SUM(size_bytes), 0)::BIGINT FROM volumes_view 
             WHERE org_id = $1 AND NOT is_deleted"
        }
        QuotaDimension::MaxVolumeAttachments => {
            "SELECT COUNT(*)::BIGINT FROM volume_attachments_view 
             WHERE org_id = $1 AND NOT is_deleted"
        }
    };

    let usage: i64 = sqlx::query_scalar(query)
        .bind(org_id.to_string())
        .fetch_one(pool)
        .await?;

    Ok(usage)
}

pub async fn check_quota(
    pool: &PgPool,
    org_id: &OrgId,
    dimension: QuotaDimension,
    requested_delta: i64,
) -> Result<Option<QuotaExceeded>, sqlx::Error> {
    let limit = get_effective_limit(pool, org_id, dimension).await?;
    let current_usage = get_current_usage(pool, org_id, dimension).await?;

    if current_usage + requested_delta > limit {
        return Ok(Some(QuotaExceeded {
            dimension: dimension.as_str().to_string(),
            limit,
            current_usage,
            requested_delta,
        }));
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dimension_as_str() {
        assert_eq!(QuotaDimension::MaxInstances.as_str(), "max_instances");
        assert_eq!(
            QuotaDimension::MaxIpv4Allocations.as_str(),
            "max_ipv4_allocations"
        );
    }

    #[test]
    fn test_default_limits() {
        assert_eq!(QuotaDimension::MaxInstances.default_limit(), 50);
        assert_eq!(QuotaDimension::MaxIpv4Allocations.default_limit(), 5);
        assert!(QuotaDimension::MaxTotalMemoryBytes.default_limit() > 0);
    }
}
