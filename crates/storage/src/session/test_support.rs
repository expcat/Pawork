//! R6 波 A v12 升级 golden 的共享种子构造（仅测试编译）。
//!
//! 种子复现 v10/v11 时代的真实写入路径：sessions 行显式携带
//! active_branch（不依赖任何 DEFAULT 'main'），branch 行显式携带
//! parent / fork 点，事件统一经 persist_event_in_transaction 落库，
//! 保证 fixture 字节与真实库 payload_json 字节一致，禁止手写伪造字节。

use pawork_domain::{
    AgentEvent, AgentEventEnvelope, ContentPart, EventId, EventSequence, Message, MessageId,
    MessageRole, RunId, SessionId, TextContent, Timestamp,
};
use rusqlite::{params, Connection};

use crate::session::event_store::persist_event_in_transaction;
use crate::session::session_tree::{load_ancestor_lineage, visible_on_lineage};
use crate::sqlite::DatabaseActor;

const SEED_TENANT: &str = "local/default";
const SEED_PRINCIPAL: &str = "local/user";
const SEED_RUN: &str = "run-r6a";

/// 一次种子动作：追加事件 / 建 fork 分支行 / 切换 active branch。
/// 与生产路径一致：fork 只插 session_branches 行，事件追加只允许落在
/// 当前 active branch 上。
#[derive(Clone)]
pub(crate) enum SeedStep {
    Append {
        branch: &'static str,
        payload: AgentEvent,
    },
    Fork {
        branch: &'static str,
        parent: &'static str,
        from_sequence: u64,
    },
    Switch {
        branch: &'static str,
    },
}

pub(crate) struct SeedScenario {
    /// fixture 文件名前缀（v12_fork_tree / v12_interleaved / v12_compaction）。
    pub name: &'static str,
    pub session: &'static str,
    pub branches: Vec<&'static str>,
    pub steps: Vec<SeedStep>,
}

fn envelope(session: &str, sequence: u64, payload: AgentEvent) -> AgentEventEnvelope {
    AgentEventEnvelope::new(
        EventId::from(format!("event-{sequence}")),
        SessionId::from(session),
        RunId::from(SEED_RUN),
        EventSequence::new(sequence),
        Timestamp::from_unix_millis(1_000 + sequence),
        payload,
    )
}

pub(crate) fn committed(message_id: &str, text: &str, role: MessageRole) -> AgentEvent {
    AgentEvent::MessageCommitted {
        message: Message {
            id: MessageId::from(message_id),
            role,
            content: vec![ContentPart::Text(TextContent {
                text: text.to_string(),
            })],
            metadata: Default::default(),
        },
    }
}

pub(crate) fn compaction_completed(summary: &str, through: u64) -> AgentEvent {
    AgentEvent::CompactionCompleted {
        summary_message_id: MessageId::from(summary),
        compacted_through: EventSequence::new(through),
    }
}

fn role_for(sequence: u64) -> MessageRole {
    if sequence % 2 == 1 {
        MessageRole::User
    } else {
        MessageRole::Assistant
    }
}

/// 在已迁移到任意 v10+ 版本的空库上按脚本落种子。
pub(crate) async fn seed_scenario(database: &DatabaseActor, scenario: &SeedScenario) {
    let session = scenario.session.to_string();
    let title = scenario.name.to_string();
    let root_session = scenario.session.to_string();
    database
        .call(move |connection| -> rusqlite::Result<()> {
            connection.execute(
                "INSERT INTO sessions(session_id, title, created_at_ms, updated_at_ms, active_branch, tenant_id, principal_id) VALUES (?1, ?2, 1, 1, 'main', ?3, ?4)",
                params![session, title, SEED_TENANT, SEED_PRINCIPAL],
            )?;
            connection.execute(
                "INSERT INTO session_branches(branch_id, session_id, head_sequence) VALUES ('main', ?1, 0)",
                params![root_session],
            )?;
            Ok(())
        })
        .await
        .expect("actor")
        .expect("seed session root");

    let mut sequence: u64 = 0;
    let mut active_branch = "main".to_string();
    for step in scenario.steps.clone() {
        match step {
            SeedStep::Fork {
                branch,
                parent,
                from_sequence,
            } => {
                let forked_from_event = format!("event-{from_sequence}");
                let session = scenario.session.to_string();
                database
                    .call(move |connection| -> rusqlite::Result<()> {
                        connection.execute(
                            "INSERT INTO session_branches(branch_id, session_id, parent_branch_id, forked_from_event_id, head_sequence) VALUES (?1, ?2, ?3, ?4, 0)",
                            params![branch, session, parent, forked_from_event],
                        )?;
                        Ok(())
                    })
                    .await
                    .expect("actor")
                    .expect("seed fork branch");
            }
            SeedStep::Switch { branch } => {
                let session = scenario.session.to_string();
                database
                    .call(move |connection| -> rusqlite::Result<()> {
                        connection.execute(
                            "UPDATE sessions SET active_branch=?1 WHERE session_id=?2",
                            params![branch, session],
                        )?;
                        Ok(())
                    })
                    .await
                    .expect("actor")
                    .expect("seed switch branch");
                active_branch = branch.to_string();
            }
            SeedStep::Append { branch, payload } => {
                assert_eq!(
                    branch, active_branch,
                    "种子脚本必须先把 active branch 切到目标分支再追加"
                );
                sequence += 1;
                let envelope = envelope(scenario.session, sequence, payload);
                let branch_id = branch.to_string();
                database
                    .call(move |connection| {
                        let transaction =
                            connection.transaction().expect("seed append transaction");
                        persist_event_in_transaction(&transaction, &branch_id, &envelope)
                            .expect("seed append persist");
                        transaction.commit().expect("seed append commit");
                    })
                    .await
                    .expect("actor");
            }
        }
    }
}

/// 提取某 branch 祖先链上的 payload_json 字节（升序），fixture 由此生成。
pub(crate) fn lineage_payload_lines(
    connection: &Connection,
    session: &str,
    branch: &str,
) -> Vec<String> {
    let lineage = load_ancestor_lineage(connection, session, branch).expect("lineage");
    let mut statement = connection
        .prepare(
            "SELECT payload_json, branch_id, sequence FROM session_events WHERE session_id=?1 ORDER BY sequence ASC",
        )
        .expect("prepare events");
    let rows = statement
        .query_map([session], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .expect("query events")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect events");
    rows.into_iter()
        .filter(|(_, event_branch, sequence)| visible_on_lineage(&lineage, event_branch, *sequence))
        .map(|(payload, _, _)| payload)
        .collect()
}

/// 种子①：fork 树——main 1–6，fork-a 自 e3 起 7–9，fork-b 自 e8 起 10–11。
pub(crate) fn fork_tree_scenario() -> SeedScenario {
    let mut steps = Vec::new();
    for sequence in 1..=6u64 {
        steps.push(SeedStep::Append {
            branch: "main",
            payload: committed(
                &format!("m-main-{sequence}"),
                &format!("main message {sequence}"),
                role_for(sequence),
            ),
        });
    }
    steps.push(SeedStep::Fork {
        branch: "fork-a",
        parent: "main",
        from_sequence: 3,
    });
    steps.push(SeedStep::Switch { branch: "fork-a" });
    for sequence in 7..=9u64 {
        steps.push(SeedStep::Append {
            branch: "fork-a",
            payload: committed(
                &format!("m-fork-a-{sequence}"),
                &format!("fork-a message {sequence}"),
                role_for(sequence),
            ),
        });
    }
    steps.push(SeedStep::Fork {
        branch: "fork-b",
        parent: "fork-a",
        from_sequence: 8,
    });
    steps.push(SeedStep::Switch { branch: "fork-b" });
    for sequence in 10..=11u64 {
        steps.push(SeedStep::Append {
            branch: "fork-b",
            payload: committed(
                &format!("m-fork-b-{sequence}"),
                &format!("fork-b message {sequence}"),
                role_for(sequence),
            ),
        });
    }
    SeedScenario {
        name: "v12_fork_tree",
        session: "r6a-fork-tree",
        branches: vec!["main", "fork-a", "fork-b"],
        steps,
    }
}

/// 种子②：多分支交错——全局 sequence 在 main 与 side（自 e1 fork）间交错。
pub(crate) fn interleaved_scenario() -> SeedScenario {
    let mut steps = vec![SeedStep::Append {
        branch: "main",
        payload: committed("m-1", "interleaved main 1", MessageRole::User),
    }];
    steps.push(SeedStep::Fork {
        branch: "side",
        parent: "main",
        from_sequence: 1,
    });
    steps.push(SeedStep::Switch { branch: "side" });
    steps.push(SeedStep::Append {
        branch: "side",
        payload: committed("m-side-2", "interleaved side 2", MessageRole::Assistant),
    });
    steps.push(SeedStep::Switch { branch: "main" });
    steps.push(SeedStep::Append {
        branch: "main",
        payload: committed("m-3", "interleaved main 3", MessageRole::User),
    });
    steps.push(SeedStep::Switch { branch: "side" });
    steps.push(SeedStep::Append {
        branch: "side",
        payload: committed("m-side-4", "interleaved side 4", MessageRole::Assistant),
    });
    steps.push(SeedStep::Switch { branch: "main" });
    steps.push(SeedStep::Append {
        branch: "main",
        payload: committed("m-5", "interleaved main 5", MessageRole::User),
    });
    steps.push(SeedStep::Switch { branch: "side" });
    steps.push(SeedStep::Append {
        branch: "side",
        payload: committed("m-side-6", "interleaved side 6", MessageRole::Assistant),
    });
    SeedScenario {
        name: "v12_interleaved",
        session: "r6a-interleaved",
        branches: vec!["main", "side"],
        steps,
    }
}

/// 种子③：压缩折叠——main 上 CompactionCompleted through=2（m-1/m-2 投影
/// 已删），摘要与后续消息保留；side 自 e4（水位之后）fork，保留祖先消息行。
pub(crate) fn compaction_scenario() -> SeedScenario {
    let mut steps = vec![
        SeedStep::Append {
            branch: "main",
            payload: committed("m-1", "compaction main 1", MessageRole::User),
        },
        SeedStep::Append {
            branch: "main",
            payload: committed("m-2", "compaction main 2", MessageRole::Assistant),
        },
        SeedStep::Append {
            branch: "main",
            payload: compaction_completed("m-summary", 2),
        },
        SeedStep::Append {
            branch: "main",
            payload: committed("m-summary", "compaction summary", MessageRole::Assistant),
        },
        SeedStep::Append {
            branch: "main",
            payload: committed("m-3", "compaction main 5", MessageRole::User),
        },
    ];
    steps.push(SeedStep::Fork {
        branch: "side",
        parent: "main",
        from_sequence: 4,
    });
    steps.push(SeedStep::Switch { branch: "side" });
    steps.push(SeedStep::Append {
        branch: "side",
        payload: committed("m-side-1", "compaction side 6", MessageRole::User),
    });
    SeedScenario {
        name: "v12_compaction",
        session: "r6a-compaction",
        branches: vec!["main", "side"],
        steps,
    }
}
