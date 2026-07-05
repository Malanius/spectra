use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;

use anyhow::{anyhow, Context as _, Result};
use pipewire as pw;
use pw::spa;
use spa::param::format::{MediaSubtype, MediaType};
use spa::pod::Pod;

/// Upper bound on buffered mono samples (~0.7 s at 48 kHz).
pub const MAX_SAMPLES: usize = 1 << 15;

#[derive(Debug, Clone)]
pub struct AudioDevice {
    pub name: String,
    pub description: String,
    pub is_sink: bool,
}

/// What to capture. `node_name: None` means the default output's monitor.
#[derive(Debug, Clone)]
pub struct CaptureTarget {
    pub node_name: Option<String>,
    pub is_sink: bool,
}

impl Default for CaptureTarget {
    fn default() -> Self {
        Self {
            node_name: None,
            is_sink: true,
        }
    }
}

struct Terminate;

/// A running PipeWire capture stream on its own thread. Mono samples are
/// pushed into `samples`; `pushed` counts total samples ever received so the
/// UI can detect a stalled/suspended stream.
pub struct Capture {
    pub samples: Arc<Mutex<VecDeque<f32>>>,
    pub sample_rate: Arc<AtomicU32>,
    pub pushed: Arc<AtomicU64>,
    sender: pw::channel::Sender<Terminate>,
    handle: Option<JoinHandle<()>>,
}

struct StreamData {
    format: spa::param::audio::AudioInfoRaw,
    samples: Arc<Mutex<VecDeque<f32>>>,
    sample_rate: Arc<AtomicU32>,
    pushed: Arc<AtomicU64>,
}

impl Capture {
    pub fn start(target: CaptureTarget) -> Result<Self> {
        let samples = Arc::new(Mutex::new(VecDeque::with_capacity(MAX_SAMPLES)));
        let sample_rate = Arc::new(AtomicU32::new(0));
        let pushed = Arc::new(AtomicU64::new(0));
        let (pw_sender, pw_receiver) = pw::channel::channel::<Terminate>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();

        let t_samples = samples.clone();
        let t_rate = sample_rate.clone();
        let t_pushed = pushed.clone();
        let handle = std::thread::Builder::new()
            .name("pw-capture".into())
            .spawn(move || {
                let data = StreamData {
                    format: Default::default(),
                    samples: t_samples,
                    sample_rate: t_rate,
                    pushed: t_pushed,
                };
                if let Err(e) = run_capture(target, data, pw_receiver, &ready_tx) {
                    let _ = ready_tx.send(Err(e.to_string()));
                }
            })
            .context("failed to spawn audio thread")?;

        let mut capture = Self {
            samples,
            sample_rate,
            pushed,
            sender: pw_sender,
            handle: Some(handle),
        };
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(capture),
            Ok(Err(e)) => {
                capture.join();
                Err(anyhow!(e))
            }
            Err(_) => {
                capture.join();
                Err(anyhow!("audio thread exited during startup"))
            }
        }
    }

    fn join(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = self.sender.send(Terminate);
            let _ = handle.join();
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.join();
    }
}

fn run_capture(
    target: CaptureTarget,
    data: StreamData,
    pw_receiver: pw::channel::Receiver<Terminate>,
    ready_tx: &mpsc::Sender<Result<(), String>>,
) -> Result<()> {
    pw::init();
    let mainloop =
        pw::main_loop::MainLoopRc::new(None).context("failed to create PipeWire main loop")?;
    let context = pw::context::ContextRc::new(&mainloop, None)
        .context("failed to create PipeWire context")?;
    let core = context
        .connect_rc(None)
        .context("failed to connect to PipeWire (is the daemon running?)")?;

    let _receiver = pw_receiver.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |_| mainloop.quit()
    });

    let mut props = pw::properties::properties! {
        "media.type" => "Audio",
        "media.category" => "Capture",
        "media.role" => "Music",
        "node.name" => "spectra",
    };
    if target.is_sink {
        // Capture the monitor of a sink (i.e. "what is playing").
        props.insert("stream.capture.sink", "true");
    }
    if let Some(name) = &target.node_name {
        props.insert("target.object", name.as_str());
    }

    let stream = pw::stream::StreamBox::new(&core, "spectra-capture", props)
        .context("failed to create capture stream")?;

    let _listener = stream
        .add_local_listener_with_user_data(data)
        .param_changed(|_, udata, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Ok((media_type, media_subtype)) = spa::param::format_utils::parse_format(param)
            else {
                return;
            };
            if media_type != MediaType::Audio || media_subtype != MediaSubtype::Raw {
                return;
            }
            if udata.format.parse(param).is_ok() {
                udata.sample_rate.store(udata.format.rate(), Ordering::Relaxed);
            }
        })
        .process(|stream, udata| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buffer.datas_mut();
            if datas.is_empty() {
                return;
            }
            let channels = udata.format.channels().max(1) as usize;
            let d = &mut datas[0];
            let n_bytes = d.chunk().size() as usize;
            let Some(bytes) = d.data() else { return };
            let bytes = &bytes[..n_bytes.min(bytes.len())];

            let mut queue = udata.samples.lock().unwrap();
            let mut count = 0u64;
            // Downmix interleaved f32 frames to mono.
            for frame in bytes.chunks_exact(4 * channels) {
                let mut acc = 0.0f32;
                for ch in frame.chunks_exact(4) {
                    acc += f32::from_le_bytes([ch[0], ch[1], ch[2], ch[3]]);
                }
                queue.push_back(acc / channels as f32);
                count += 1;
            }
            let overflow = queue.len().saturating_sub(MAX_SAMPLES);
            if overflow > 0 {
                queue.drain(..overflow);
            }
            drop(queue);
            udata.pushed.fetch_add(count, Ordering::Relaxed);
        })
        .register()
        .context("failed to register stream listener")?;

    // Negotiate 32-bit float; let PipeWire pick rate and channel count.
    let mut audio_info = spa::param::audio::AudioInfoRaw::new();
    audio_info.set_format(spa::param::audio::AudioFormat::F32LE);
    let values: Vec<u8> = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(spa::pod::Object {
            type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
            id: spa::param::ParamType::EnumFormat.as_raw(),
            properties: audio_info.into(),
        }),
    )
    .map_err(|e| anyhow!("failed to serialize format pod: {e:?}"))?
    .0
    .into_inner();
    let mut params = [Pod::from_bytes(&values).ok_or_else(|| anyhow!("invalid format pod"))?];

    stream
        .connect(
            spa::utils::Direction::Input,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        )
        .context("failed to connect capture stream")?;

    let _ = ready_tx.send(Ok(()));
    mainloop.run();
    Ok(())
}

/// List audio sinks (playback devices, captured via monitor) and sources
/// (microphones etc.) currently known to PipeWire.
pub fn enumerate_devices() -> Result<Vec<AudioDevice>> {
    pw::init();
    let mainloop =
        pw::main_loop::MainLoopRc::new(None).context("failed to create PipeWire main loop")?;
    let context = pw::context::ContextRc::new(&mainloop, None)
        .context("failed to create PipeWire context")?;
    let core = context
        .connect_rc(None)
        .context("failed to connect to PipeWire (is the daemon running?)")?;
    let registry = core.get_registry().context("failed to get registry")?;

    let devices = Rc::new(RefCell::new(Vec::<AudioDevice>::new()));
    let devices_cb = devices.clone();
    let _reg_listener = registry
        .add_listener_local()
        .global(move |global| {
            let Some(props) = global.props else { return };
            let Some(class) = props.get("media.class") else {
                return;
            };
            let is_sink = match class {
                "Audio/Sink" => true,
                "Audio/Source" => false,
                _ => return,
            };
            let Some(name) = props.get("node.name") else {
                return;
            };
            let description = props
                .get("node.description")
                .or_else(|| props.get("node.nick"))
                .unwrap_or(name)
                .to_string();
            devices_cb.borrow_mut().push(AudioDevice {
                name: name.to_string(),
                description,
                is_sink,
            });
        })
        .register();

    // Round-trip: quit the loop once the server has sent everything above.
    let pending = core.sync(0).context("sync failed")?;
    let ml = mainloop.clone();
    let _core_listener = core
        .add_listener_local()
        .done(move |id, seq| {
            if id == pw::core::PW_ID_CORE && seq == pending {
                ml.quit();
            }
        })
        .register();

    mainloop.run();

    let mut devs = devices.borrow().clone();
    devs.sort_by(|a, b| b.is_sink.cmp(&a.is_sink).then(a.description.cmp(&b.description)));
    Ok(devs)
}
