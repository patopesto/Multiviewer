use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use super::Compositor;

const SLOTS: usize = 3;
const QUERIES_PER_SLOT: u32 = 2;
const SMOOTHING: f32 = 0.2;
/// `resolve_query_set` wants its destination offset aligned to this, not to 8.
const RESOLVE_ALIGN: u64 = wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT;

/// GPU work queued by the compositor during one engine tick; reset by `begin_frame`.
#[derive(Clone, Copy, Default)]
pub struct FrameStats {
    pub draws: u32,
    pub verts: u32,
    pub uploads: u32,
    pub upload_bytes: u64,
    pub upload_fallbacks: u32,
    pub textures_created: u32,
    pub canvas_submits: u32,
}

impl FrameStats {
    /// One source frame handed to the GPU. `fallback` marks a frame that could
    /// not use its staging buffer and went through `queue.write_texture`.
    pub fn upload(&mut self, bytes: u64, fallback: bool) {
        self.uploads += 1;
        self.upload_bytes += bytes;
        if fallback {
            self.upload_fallbacks += 1;
        }
    }

    pub fn texture_created(&mut self) {
        self.textures_created += 1;
    }

    /// One canvas render pass recorded, with what it drew.
    pub fn canvas_pass(&mut self, draws: u32, verts: u32) {
        self.canvas_submits = 1;
        self.draws = draws;
        self.verts = verts;
    }

    /// Emit the per-tick summary. This field list is what the trace schema is.
    pub fn report(&self, sources: usize, tick_ms: f32, gpu_ms: Option<f32>) {
        tracing::debug!(
            tick_ms,
            sources,
            draws = self.draws,
            verts = self.verts,
            uploads = self.uploads,
            upload_bytes = self.upload_bytes,
            upload_fallbacks = self.upload_fallbacks,
            textures_created = self.textures_created,
            canvas_submits = self.canvas_submits,
            gpu_ms = ?gpu_ms,
            "tick"
        );
    }
}

/// Per-pass GPU duration, read back through a small ring so sampling never
/// stalls the tick.
pub struct GpuTimers {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    period_ns: f32,
    slots: Vec<Slot>,
    smoothed_ms: Option<f32>,
}

struct Slot {
    buffer: wgpu::Buffer,
    state: SlotState,
}

/// A slot is only handed out from `Idle`, which means its buffer is unmapped.
enum SlotState {
    Idle,
    Pending(Receiver<Result<(), wgpu::BufferAsyncError>>, Instant),
}

/// What a poll of a pending slot decided.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SlotOutcome {
    /// Still mapping; leave the slot alone.
    Waiting,
    /// The values are readable; the caller must unmap afterwards.
    Ready,
    /// The map failed, was dropped, or never completed; the caller must unmap.
    Lost,
}

/// A map that has not completed in this long is treated as lost: a slot left
/// pending for good starves the timer ring until the next project load.
const STUCK_MAP: Duration = Duration::from_secs(1);

/// Polls a pending slot and returns it to `Idle` unless it is still waiting.
fn poll_slot(state: &mut SlotState, now: Instant) -> SlotOutcome {
    let SlotState::Pending(rx, since) = state else {
        return SlotOutcome::Waiting;
    };
    let outcome = match rx.try_recv() {
        Ok(Ok(())) => SlotOutcome::Ready,
        Ok(Err(_)) | Err(TryRecvError::Disconnected) => SlotOutcome::Lost,
        Err(TryRecvError::Empty) => {
            if now.duration_since(*since) < STUCK_MAP {
                return SlotOutcome::Waiting;
            }
            SlotOutcome::Lost
        }
    };
    *state = SlotState::Idle;
    return outcome;
}

impl GpuTimers {
    /// `None` when the adapter cannot timestamp passes.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        let queries = SLOTS as u32 * QUERIES_PER_SLOT;
        let set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("canvas-timestamps"),
            count: queries,
            ty: wgpu::QueryType::Timestamp,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("canvas-timestamps-resolve"),
            size: SLOTS as u64 * RESOLVE_ALIGN,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let slots = (0..SLOTS)
            .map(|_| Slot {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("canvas-timestamps-readback"),
                    size: QUERIES_PER_SLOT as u64 * 8,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                state: SlotState::Idle,
            })
            .collect();
        return Some(Self {
            set,
            resolve,
            period_ns: queue.get_timestamp_period(),
            slots,
            smoothed_ms: None,
        });
    }

    /// A slot whose previous readback has landed.
    pub fn free_slot(&self) -> Option<usize> {
        return self.slots.iter().position(|slot| matches!(slot.state, SlotState::Idle));
    }

    pub fn writes(&self, slot: usize) -> wgpu::RenderPassTimestampWrites<'_> {
        let (range, _) = slot_range(slot);
        return wgpu::RenderPassTimestampWrites {
            query_set: &self.set,
            beginning_of_pass_write_index: Some(range.start),
            end_of_pass_write_index: Some(range.end - 1),
        };
    }

    /// Resolve `slot`'s timestamps into its readback buffer; the pass they
    /// belong to must already be recorded in `encoder`.
    pub fn encode_readback(&self, encoder: &mut wgpu::CommandEncoder, slot: usize) {
        let (range, offset) = slot_range(slot);
        encoder.resolve_query_set(&self.set, range, &self.resolve, offset);
        encoder.copy_buffer_to_buffer(&self.resolve, offset, &self.slots[slot].buffer, 0, QUERIES_PER_SLOT as u64 * 8);
    }

    /// Start the map for `slot`; call after the submission that copies into it.
    pub fn arm(&mut self, slot: usize) {
        if !matches!(self.slots[slot].state, SlotState::Idle) {
            tracing::warn!(slot, "timestamp slot armed while not idle");
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.slots[slot].buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.slots[slot].state = SlotState::Pending(rx, Instant::now());
    }

    /// Smoothed pass duration in ms, reaping any readback that completed.
    pub fn gpu_ms(&mut self) -> Option<f32> {
        let now = Instant::now();
        for slot in &mut self.slots {
            match poll_slot(&mut slot.state, now) {
                SlotOutcome::Waiting => continue,
                SlotOutcome::Lost => {
                    slot.buffer.unmap();
                    tracing::debug!("timestamp map lost, slot unmapped");
                    continue;
                }
                SlotOutcome::Ready => {}
            }
            let Ok(view) = slot.buffer.slice(..).get_mapped_range() else {
                slot.buffer.unmap();
                continue;
            };
            let start = u64::from_le_bytes(view[0..8].try_into().expect("8 bytes"));
            let end = u64::from_le_bytes(view[8..16].try_into().expect("8 bytes"));
            drop(view);
            slot.buffer.unmap();
            if end <= start {
                continue;
            }
            let ms = (end - start) as f32 * self.period_ns / 1_000_000.0;
            self.smoothed_ms = Some(match self.smoothed_ms {
                Some(prev) => prev + (ms - prev) * SMOOTHING,
                None => ms,
            });
        }
        return self.smoothed_ms;
    }
}

impl Compositor {
    pub fn frame_stats(&self) -> FrameStats {
        return self.frame_stats;
    }

    /// Smoothed GPU duration of the last canvas passes, if the adapter can timestamp them.
    pub fn gpu_ms(&mut self) -> Option<f32> {
        return self.timers.as_mut().and_then(GpuTimers::gpu_ms);
    }
}

/// Query indices and byte offset a slot owns inside the shared query set.
fn slot_range(slot: usize) -> (std::ops::Range<u32>, u64) {
    let start = slot as u32 * QUERIES_PER_SLOT;
    return (start..start + QUERIES_PER_SLOT, slot as u64 * RESOLVE_ALIGN);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pending_slot_is_not_ready_and_a_lost_map_returns_it_to_idle() {
        let now = Instant::now();
        let (tx, rx) = mpsc::channel();
        let mut state = SlotState::Pending(rx, now);
        assert_eq!(poll_slot(&mut state, now), SlotOutcome::Waiting);

        drop(tx);
        assert_eq!(poll_slot(&mut state, now), SlotOutcome::Lost);
        assert!(matches!(state, SlotState::Idle));
    }

    #[test]
    fn a_completed_map_is_ready_and_clears_the_slot() {
        let now = Instant::now();
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(())).expect("receiver alive");
        let mut state = SlotState::Pending(rx, now);
        assert_eq!(poll_slot(&mut state, now), SlotOutcome::Ready);
        assert!(matches!(state, SlotState::Idle));
    }

    #[test]
    fn a_map_that_never_completes_is_recovered_after_the_stall_timeout() {
        let now = Instant::now();
        let (tx, rx) = mpsc::channel();
        let mut state = SlotState::Pending(rx, now - STUCK_MAP * 2);
        assert_eq!(poll_slot(&mut state, now), SlotOutcome::Lost);
        assert!(matches!(state, SlotState::Idle));
        drop(tx);
    }

    #[test]
    fn slots_own_disjoint_queries_and_aligned_offsets() {
        let mut queries = Vec::new();
        let mut offsets = Vec::new();
        for slot in 0..SLOTS {
            let (range, offset) = slot_range(slot);
            assert_eq!(range.start, slot as u32 * QUERIES_PER_SLOT);
            assert_eq!(range.end - range.start, QUERIES_PER_SLOT);
            assert_eq!(offset % RESOLVE_ALIGN, 0);
            queries.extend(range);
            offsets.push(offset);
        }
        queries.sort();
        queries.dedup();
        assert_eq!(queries.len(), SLOTS * QUERIES_PER_SLOT as usize);
        assert_eq!(offsets.len(), SLOTS);
    }

    /// A tick that only prefetched reports uploads without a canvas pass, and a
    /// canvas tick overwrites the draw counts instead of accumulating them.
    #[test]
    fn stats_record_uploads_and_canvas_pass() {
        let mut stats = FrameStats::default();
        stats.upload(8_294_400, false);
        stats.upload(2_073_600, true);
        stats.texture_created();
        assert_eq!(stats.uploads, 2);
        assert_eq!(stats.upload_bytes, 10_368_000);
        assert_eq!(stats.upload_fallbacks, 1);
        assert_eq!(stats.textures_created, 1);

        stats.canvas_pass(91, 364);
        stats.canvas_pass(7, 28);
        assert_eq!((stats.draws, stats.verts), (7, 28));
        assert_eq!(stats.canvas_submits, 1);

        stats = FrameStats::default();
        assert_eq!((stats.uploads, stats.draws), (0, 0));
    }
}
