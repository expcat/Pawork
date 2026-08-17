//! resume disposition 计算（L1 定向测试）。

use pawork_protocol::GlobalSequence;
use pawork_protocol::{compute_resume_disposition, ResumeDisposition};

#[test]
fn up_to_date_when_last_equals_current() {
    assert_eq!(
        compute_resume_disposition(GlobalSequence(10), GlobalSequence(20), GlobalSequence(20),),
        ResumeDisposition::UpToDate {
            current_sequence: GlobalSequence(20),
        }
    );
}

#[test]
fn replay_when_within_history() {
    assert_eq!(
        compute_resume_disposition(GlobalSequence(10), GlobalSequence(20), GlobalSequence(15),),
        ResumeDisposition::Replay {
            from_sequence: GlobalSequence(16),
            through_sequence: GlobalSequence(20),
        }
    );
}

#[test]
fn replay_from_exactly_earliest() {
    assert_eq!(
        compute_resume_disposition(GlobalSequence(10), GlobalSequence(20), GlobalSequence(9),),
        ResumeDisposition::Replay {
            from_sequence: GlobalSequence(10),
            through_sequence: GlobalSequence(20),
        }
    );
}

#[test]
fn snapshot_required_when_behind_retention() {
    assert_eq!(
        compute_resume_disposition(GlobalSequence(10), GlobalSequence(20), GlobalSequence(8),),
        ResumeDisposition::SnapshotRequired {
            earliest_available_sequence: GlobalSequence(10),
        }
    );
}

#[test]
fn snapshot_required_when_client_is_ahead() {
    assert_eq!(
        compute_resume_disposition(GlobalSequence(10), GlobalSequence(20), GlobalSequence(21),),
        ResumeDisposition::SnapshotRequired {
            earliest_available_sequence: GlobalSequence(10),
        }
    );
}

#[test]
fn snapshot_required_when_history_is_empty_and_client_behind() {
    assert_eq!(
        compute_resume_disposition(GlobalSequence(5), GlobalSequence(5), GlobalSequence(3)),
        ResumeDisposition::SnapshotRequired {
            earliest_available_sequence: GlobalSequence(5),
        }
    );
}

#[test]
fn up_to_date_when_no_events_at_all() {
    assert_eq!(
        compute_resume_disposition(GlobalSequence(0), GlobalSequence(0), GlobalSequence(0)),
        ResumeDisposition::UpToDate {
            current_sequence: GlobalSequence(0),
        }
    );
}

#[test]
fn replay_when_history_starts_at_zero() {
    assert_eq!(
        compute_resume_disposition(GlobalSequence(0), GlobalSequence(5), GlobalSequence(0)),
        ResumeDisposition::Replay {
            from_sequence: GlobalSequence(1),
            through_sequence: GlobalSequence(5),
        }
    );
}
