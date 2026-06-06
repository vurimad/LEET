use std::sync::{Arc, Mutex};

use leet_jobs2::{JobSystemConfig, LeetJobSystem, Priority};

use super::super::{
    execute_graph_sequential_gpu_order, FrameResourceAllocator, RenderGraphCache,
    RenderGraphCameraBuildData, RenderGraphResult, RenderGraphShapeHash,
    RenderGraphShapeHashBuilder, RenderNodeCommandListUsage, RenderNodeGraphFactory,
    RenderNodeImpl, RenderNodeImplContext, RenderNodeKind, RenderNodeProcessState,
    RenderNodeSubtype, ResourceAllocatorPhase,
};
use leet_jobs2::Builder as RenderJobBuilder;

#[derive(Clone)]
struct CacheTestNode {
    name: &'static str,
    log: Arc<Mutex<Vec<&'static str>>>,
}

impl CacheTestNode {
    fn new(name: &'static str, log: Arc<Mutex<Vec<&'static str>>>) -> Self {
        Self { name, log }
    }
}

impl RenderNodeImpl for CacheTestNode {
    fn name(&self) -> &str {
        self.name
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::None
    }

    fn execute(
        &self,
        _rctx: &mut RenderNodeImplContext<'_>,
        _jobs: &mut RenderJobBuilder,
    ) -> RenderGraphResult<()> {
        self.log.lock().unwrap().push(self.name);
        Ok(())
    }
}

struct JobHarness {
    jobs: LeetJobSystem,
}

impl JobHarness {
    fn new() -> Self {
        Self {
            jobs: LeetJobSystem::new(JobSystemConfig {
                max_threads: 1,
                ..JobSystemConfig::default()
            }),
        }
    }

    fn builder(&self) -> RenderJobBuilder {
        self.jobs.create_builder(Priority::RenderPath)
    }
}

impl Drop for JobHarness {
    fn drop(&mut self) {
        self.jobs.shutdown();
    }
}

fn hash(value: u64) -> RenderGraphShapeHash {
    let mut builder = RenderGraphShapeHashBuilder::new();
    builder.append_u64(value);
    builder.finish()
}

fn build_graph(
    name: &'static str,
    log: Arc<Mutex<Vec<&'static str>>>,
) -> super::super::FinalRenderNodeGraph {
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            CacheTestNode::new(name, log),
        )
        .unwrap();
    factory.finish().unwrap()
}

#[test]
fn cache_hit_requires_same_graph_shape_hash() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut cache = RenderGraphCache::with_capacity(2);
    let first_hash = hash(1);
    let second_hash = hash(2);

    {
        let lookup = cache.get_graph(first_hash, 1, 10, false).unwrap();
        assert!(lookup.needs_rebuild);
        lookup
            .entry
            .set_final_graph(build_graph("first", Arc::clone(&log)));
    }

    {
        let lookup = cache.get_graph(first_hash, 1, 11, false).unwrap();
        assert!(!lookup.needs_rebuild);
        assert_eq!(lookup.entry.shape_hash(), first_hash);
    }

    {
        let lookup = cache.get_graph(second_hash, 1, 12, false).unwrap();
        assert!(lookup.needs_rebuild);
        assert_eq!(lookup.entry.shape_hash(), second_hash);
    }
}

#[test]
fn cache_hit_requires_same_camera_setup_count() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut cache = RenderGraphCache::with_capacity(2);
    let graph_hash = hash(11);

    {
        let lookup = cache.get_graph(graph_hash, 1, 1, false).unwrap();
        lookup.entry.set_final_graph(build_graph("one-camera", log));
    }

    let lookup = cache.get_graph(graph_hash, 2, 2, false).unwrap();
    assert!(lookup.needs_rebuild);
    assert_eq!(lookup.entry.camera_setup_count(), 2);
}

#[test]
fn cache_miss_reuses_oldest_entry() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut cache = RenderGraphCache::with_capacity(2);
    let first = hash(100);
    let second = hash(200);
    let third = hash(300);

    {
        let lookup = cache.get_graph(first, 1, 10, false).unwrap();
        assert_eq!(lookup.entry_index, 0);
        lookup
            .entry
            .set_final_graph(build_graph("first", Arc::clone(&log)));
    }
    {
        let lookup = cache.get_graph(second, 1, 20, false).unwrap();
        assert_eq!(lookup.entry_index, 1);
        lookup
            .entry
            .set_final_graph(build_graph("second", Arc::clone(&log)));
    }
    {
        let lookup = cache.get_graph(first, 1, 30, false).unwrap();
        assert_eq!(lookup.entry_index, 0);
        assert!(!lookup.needs_rebuild);
    }
    {
        let lookup = cache.get_graph(third, 1, 40, false).unwrap();
        assert_eq!(lookup.entry_index, 1);
        assert!(lookup.needs_rebuild);
        assert_eq!(lookup.entry.shape_hash(), third);
    }
}

#[test]
fn per_camera_node_storage_survives_temporary_topology_clear() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut cache = RenderGraphCache::with_capacity(1);
    let graph_hash = hash(7);
    let camera_hash = hash(70);

    let lookup = cache.get_graph(graph_hash, 1, 1, false).unwrap();
    let camera_data =
        RenderGraphCameraBuildData::from_built_graph(camera_hash, build_graph("camera", log));
    lookup.entry.set_camera_build_data(0, camera_data).unwrap();

    assert_eq!(
        lookup.entry.camera_build_data()[0]
            .temporary_graph()
            .node_count(),
        1
    );
    assert_eq!(lookup.entry.camera_build_data()[0].node_impls().len(), 1);

    lookup.entry.post_build_clear();

    assert_eq!(
        lookup.entry.camera_build_data()[0]
            .temporary_graph()
            .node_count(),
        0
    );
    assert_eq!(lookup.entry.camera_build_data()[0].node_impls().len(), 1);
}

#[test]
fn final_merged_graph_remains_executable_after_camera_temp_topology_clears() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let temp_log = Arc::new(Mutex::new(Vec::new()));
    let mut cache = RenderGraphCache::with_capacity(1);
    let graph_hash = hash(9);

    let lookup = cache.get_graph(graph_hash, 1, 1, false).unwrap();
    lookup
        .entry
        .set_final_graph(build_graph("final", Arc::clone(&log)));
    lookup
        .entry
        .set_camera_build_data(
            0,
            RenderGraphCameraBuildData::from_built_graph(hash(90), build_graph("temp", temp_log)),
        )
        .unwrap();
    lookup.entry.post_build_clear();

    let final_graph = lookup.entry.final_graph().unwrap();
    let mut allocator = FrameResourceAllocator::new();
    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();
    let jobs = JobHarness::new();
    let mut builder = jobs.builder();
    let mut state = RenderNodeProcessState::new();

    execute_graph_sequential_gpu_order(
        final_graph.graph(),
        final_graph.impl_store(),
        final_graph.command_group_store(),
        &mut state,
        &mut allocator,
        &mut builder,
    )
    .unwrap();

    assert_eq!(log.lock().unwrap().as_slice(), &["final"]);
}

#[test]
fn cache_does_not_store_transient_gpu_resources() {
    let mut cache = RenderGraphCache::with_capacity(1);
    let lookup = cache.get_graph(hash(5), 0, 1, false).unwrap();

    assert!(!lookup.entry.has_transient_gpu_resources());
}
