//! Project / ProjectFile / ProjectTimeline 卷宗模型
//!
//! 实施 spec §2.1：通用 Project 层（kind = 自由字符串，由调用方约定）。
//! attune-core 不约束 kind 取值集合 — 行业层（attune-pro 系列插件）通过
//! metadata_encrypted 持有 opaque blob，attune-core 不解析它的内部结构，
//! 只负责存取 + 时间线 + 文件归属。

use chrono::Utc;
use rusqlite::params;
use uuid::Uuid;

use crate::error::Result;
use crate::store::types::*;
use crate::store::Store;

impl Store {
    // --- project ---

    pub fn create_project(&self, title: &str, kind: &str) -> Result<Project> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().timestamp();
        self.conn.execute(
            "INSERT INTO project (id, title, kind, metadata_encrypted, created_at, updated_at, archived) \
             VALUES (?1, ?2, ?3, NULL, ?4, ?4, 0)",
            params![&id, title, kind, now],
        )?;
        Ok(Project {
            id,
            title: title.to_string(),
            kind: kind.to_string(),
            metadata_encrypted: None,
            created_at: now,
            updated_at: now,
            archived: false,
        })
    }

    pub fn get_project(&self, id: &str) -> Result<Option<Project>> {
        let result = self.conn.query_row(
            "SELECT id, title, kind, metadata_encrypted, created_at, updated_at, archived \
             FROM project WHERE id = ?1",
            params![id],
            |row| {
                Ok(Project {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    kind: row.get::<_, String>(2)?,
                    metadata_encrypted: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                    archived: row.get::<_, i64>(6)? != 0,
                })
            },
        );
        match result {
            Ok(p) => Ok(Some(p)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn list_projects(&self, include_archived: bool) -> Result<Vec<Project>> {
        let sql = if include_archived {
            "SELECT id, title, kind, metadata_encrypted, created_at, updated_at, archived \
             FROM project ORDER BY updated_at DESC"
        } else {
            "SELECT id, title, kind, metadata_encrypted, created_at, updated_at, archived \
             FROM project WHERE archived = 0 ORDER BY updated_at DESC"
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([], |row| {
            Ok(Project {
                id: row.get(0)?,
                title: row.get(1)?,
                kind: row.get::<_, String>(2)?,
                metadata_encrypted: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
                archived: row.get::<_, i64>(6)? != 0,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.into())
    }

    pub fn add_file_to_project(
        &self,
        project_id: &str,
        file_id: &str,
        role: &str,
    ) -> Result<()> {
        let now = Utc::now().timestamp();
        self.conn.execute(
            "INSERT OR REPLACE INTO project_file (project_id, file_id, role, added_at) \
             VALUES (?1, ?2, ?3, ?4)",
            params![project_id, file_id, role, now],
        )?;
        Ok(())
    }

    pub fn list_files_for_project(&self, project_id: &str) -> Result<Vec<ProjectFile>> {
        let mut stmt = self.conn.prepare(
            "SELECT project_id, file_id, role, added_at FROM project_file \
             WHERE project_id = ?1 ORDER BY added_at ASC",
        )?;
        let rows = stmt.query_map(params![project_id], |row| {
            Ok(ProjectFile {
                project_id: row.get(0)?,
                file_id: row.get(1)?,
                role: row.get(2)?,
                added_at: row.get(3)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.into())
    }

    pub fn append_timeline(
        &self,
        project_id: &str,
        event_type: &str,
        payload_encrypted: Option<&[u8]>,
    ) -> Result<()> {
        let now = Utc::now().timestamp_millis();
        self.conn.execute(
            "INSERT INTO project_timeline (project_id, ts, event_type, payload_encrypted) \
             VALUES (?1, ?2, ?3, ?4)",
            params![project_id, now, event_type, payload_encrypted],
        )?;
        Ok(())
    }

    pub fn list_timeline(&self, project_id: &str) -> Result<Vec<ProjectTimelineEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT project_id, ts, event_type, payload_encrypted FROM project_timeline \
             WHERE project_id = ?1 ORDER BY ts ASC",
        )?;
        let rows = stmt.query_map(params![project_id], |row| {
            Ok(ProjectTimelineEntry {
                project_id: row.get(0)?,
                ts_ms: row.get(1)?,
                event_type: row.get(2)?,
                payload_encrypted: row.get(3)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_get_project() {
        let store = Store::open_memory().expect("open memory store");
        let p = store
            .create_project("Project Alpha", "generic")
            .expect("create project");
        assert_eq!(p.title, "Project Alpha");
        assert_eq!(p.kind, "generic");

        let fetched = store.get_project(&p.id).expect("get").expect("some");
        assert_eq!(fetched.id, p.id);
        assert_eq!(fetched.title, "Project Alpha");
        assert_eq!(fetched.kind, "generic");
        assert!(!fetched.archived);
    }

    #[test]
    fn list_projects_excludes_archived_by_default() {
        let store = Store::open_memory().expect("open");
        let p1 = store
            .create_project("Active", "generic")
            .expect("c1");
        let _p2 = store
            .create_project("Other", "topic")
            .expect("c2");

        let active = store.list_projects(false).expect("list");
        assert_eq!(active.len(), 2);

        // 归档 p1 — 直接 SQL（正式 archive_project fn 留 Phase B）
        store
            .conn
            .execute(
                "UPDATE project SET archived = 1 WHERE id = ?1",
                params![&p1.id],
            )
            .expect("archive update");

        let active = store.list_projects(false).expect("list active");
        assert_eq!(active.len(), 1);
        let all = store.list_projects(true).expect("list all");
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn add_file_and_list() {
        let store = Store::open_memory().expect("open");
        let p = store
            .create_project("Project A", "generic")
            .expect("c");
        store
            .add_file_to_project(&p.id, "file-uuid-001", "evidence")
            .expect("add evidence");
        store
            .add_file_to_project(&p.id, "file-uuid-002", "pleading")
            .expect("add pleading");

        let files = store.list_files_for_project(&p.id).expect("list");
        assert_eq!(files.len(), 2);
        let roles: Vec<_> = files.iter().map(|f| f.role.as_str()).collect();
        assert!(roles.contains(&"evidence"));
        assert!(roles.contains(&"pleading"));
    }

    #[test]
    fn timeline_append_and_list() {
        let store = Store::open_memory().expect("open");
        let p = store
            .create_project("Project B", "generic")
            .expect("c");
        store
            .append_timeline(&p.id, "evidence_added", None)
            .expect("append 1");
        store
            .append_timeline(&p.id, "rpa_call", Some(b"opaque payload"))
            .expect("append 2");

        let timeline = store.list_timeline(&p.id).expect("list");
        assert_eq!(timeline.len(), 2);
        assert_eq!(timeline[0].event_type, "evidence_added");
        assert_eq!(timeline[1].event_type, "rpa_call");
        assert_eq!(
            timeline[1].payload_encrypted.as_deref(),
            Some(b"opaque payload" as &[u8])
        );
    }
}
