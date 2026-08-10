use super::*;

    #[test]
    fn index_scheduler_coalesces_a_burst_and_keeps_one_dirty_rerun() {
        let workspace = std::path::PathBuf::from("/workspace");
        let mut scheduler = IndexScheduler::default();

        let first = scheduler.request(workspace.clone(), true);
        let second = scheduler.request(workspace.clone(), true);
        assert!(first.spawn_worker);
        assert!(!second.spawn_worker);

        let active = scheduler.take_pending().expect("coalesced pass");
        assert_eq!(active.generation, second.generation);
        assert!(active.debounce);

        // An event that arrives after the pass was claimed becomes one dirty
        // rerun, regardless of how many more events join it.
        let dirty = scheduler.request(workspace.clone(), true);
        let latest = scheduler.request(workspace, true);
        assert!(!dirty.spawn_worker);
        assert!(!latest.spawn_worker);
        assert!(scheduler.finish_pass(active.generation, None));

        let rerun = scheduler.take_pending().expect("dirty rerun");
        assert_eq!(rerun.generation, latest.generation);
        assert!(!scheduler.finish_pass(rerun.generation, None));
        assert!(!scheduler.worker_running);
    }

    #[test]
    fn index_scheduler_immediate_request_dominates_debounce() {
        let workspace = std::path::PathBuf::from("/workspace");
        let mut scheduler = IndexScheduler::default();

        scheduler.request(workspace.clone(), true);
        let immediate = scheduler.request(workspace.clone(), false);
        scheduler.request(workspace, true);

        let pending = scheduler.take_pending().expect("pending pass");
        assert!(pending.generation > immediate.generation);
        assert!(!pending.debounce);
    }

    #[test]
    fn index_scheduler_abort_completes_waiters_and_can_restart() {
        let workspace = std::path::PathBuf::from("/workspace");
        let mut scheduler = IndexScheduler::default();

        let first = scheduler.request(workspace.clone(), false);
        let first_worker_id = first.worker_id.expect("first worker id");
        let _active = scheduler.take_pending().expect("active pass");
        let queued = scheduler.request(workspace.clone(), true);
        assert!(scheduler.abort_worker(first_worker_id, "cancelled"));
        assert_eq!(
            scheduler.completion_for(first.generation),
            Some(Err("cancelled".to_string()))
        );
        assert_eq!(
            scheduler.completion_for(queued.generation),
            Some(Err("cancelled".to_string()))
        );

        let restarted = scheduler.request(workspace, true);
        assert!(restarted.spawn_worker);
        assert_eq!(
            scheduler.take_pending().expect("restart pass").generation,
            restarted.generation
        );
    }

    #[test]
    fn index_scheduler_completed_worker_cannot_abort_a_new_worker_on_drop() {
        let workspace = std::path::PathBuf::from("/workspace");
        let mut scheduler = IndexScheduler::default();

        let first = scheduler.request(workspace.clone(), false);
        let first_worker_id = first.worker_id.expect("first worker id");
        let pass = scheduler.take_pending().expect("first pass");
        assert!(!scheduler.finish_pass(pass.generation, None));

        let second = scheduler.request(workspace, false);
        let second_worker_id = second.worker_id.expect("second worker id");
        assert_ne!(first_worker_id, second_worker_id);
        // This models worker A's Drop running after worker B has started. The
        // old lease must not cancel or complete B's generation.
        assert!(!scheduler.abort_worker(first_worker_id, "stale worker drop"));
        assert!(scheduler.worker_running);
        assert_eq!(scheduler.active_worker_id, Some(second_worker_id));
        assert_eq!(scheduler.completion_for(second.generation), None);
    }

