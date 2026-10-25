//! Command-line batch rendering: `rust_maya --render scene.rma [options]` renders frames
//! with the path tracer without opening a window (works on a headless server / VPS).

use crate::app::App;
use crate::render_view::{RenderCam, frame_path};

pub const HELP: &str = "\
Rust Maya batch renderer

USAGE:
    rust_maya --render <scene.rma> [options]

Renders with the scene's saved Render Settings and render camera unless overridden.

OPTIONS:
    -cam <name>        Camera node to render through (default: the saved render camera,
                       else the first camera in the scene)
    -f <frame>         Render one frame (default: the frame the scene was saved on)
    -s <frame>         Start frame of a sequence
    -e <frame>         End frame of a sequence (default: same as -s)
    -all               Render the playback range
    -o <path>          Output image (default: <scene folder>/<scene name>.png). Sequences are
                       numbered name.0001.png. End in .exr for linear 32-bit float output.
    -x <px> -y <px>    Resolution
    -spp <n>           Samples per pixel
    -b <n>             Maximum bounces
    -env <k>           Sky intensity
    -exp <stops>       Exposure
    -tm srgb|filmic    Tone map
    -dn on|off         Denoise
    -bg env|black      Background
    -mb on|off         Motion blur
    -shutter <frames>  Shutter length in frames (turns motion blur on; 0.5 = 180°)
    -passes            Also save the passes: albedo, normal, depth, object id, material id,
                       diffuse direct, diffuse indirect, specular, emission
                       (name_<pass>[.####].png/.exr)
    -mattes <list>     Antialiased ID mattes (name_matte_<name>[.####].png/.exr): `all` (one per
                       object), `materials` (one per material), or comma-separated object, group
                       or material names
    -step <n>          Render every n-th frame of the range
    -chunk <i>/<n>     Render only part i of n of the range (contiguous blocks), so n machines
                       can share a sequence: run -chunk 1/4 … -chunk 4/4 with the same options
    -resume            Skip frames whose image already exists (finished frames are written
                       under a temporary name first, so interrupted frames are redone)
    -mel \"<commands>\"  Run MEL commands after loading, before rendering
    -q                 Quiet: only errors
    -h, --help         This help

EXAMPLES:
    rust_maya --render shot.rma -s 1 -e 120 -o renders/shot.png
    rust_maya --render shot.rma -f 48 -x 1920 -y 1080 -spp 256 -mb on -passes -o out/f48.exr
    rust_maya --render shot.rma -all -chunk 2/3 -resume -mattes hero,materials -o /srv/shot.png

Exit codes: 0 = success, 1 = render or file error, 2 = bad arguments.
";

/// Entry point when the first argument is `--render`. Returns the process exit code.
pub fn run(args: &[String]) -> i32 {
    match run_inner(args) {
        Ok(()) => 0,
        Err((code, msg)) => {
            eprintln!("error: {msg}");
            if code == 2 {
                eprintln!("Run `rust_maya --help` for usage.");
            }
            code
        }
    }
}

type Fail = (i32, String);

fn usage(m: impl Into<String>) -> Fail {
    (2, m.into())
}

fn run_inner(args: &[String]) -> Result<(), Fail> {
    let scene_path = args.first().filter(|a| !a.starts_with('-')).ok_or_else(|| usage("--render needs a scene file"))?;
    let mut opts = std::collections::HashMap::<String, String>::new();
    let mut flags = std::collections::HashSet::<String>::new();
    let takes_value = ["-cam", "-camera", "-f", "-s", "-e", "-o", "-x", "-y", "-spp", "-b", "-env", "-exp", "-tm", "-dn", "-bg", "-mb", "-shutter", "-mel", "-mattes", "-step", "-chunk"];
    let switches = ["-all", "-passes", "-q", "-resume"];
    let mut i = 1;
    while i < args.len() {
        let a = args[i].as_str();
        let key = a.strip_prefix('-').map(|k| format!("-{}", k.trim_start_matches('-'))).unwrap_or_default();
        if takes_value.contains(&key.as_str()) {
            let v = args.get(i + 1).ok_or_else(|| usage(format!("{a} needs a value")))?;
            opts.insert(key, v.clone());
            i += 2;
        } else if switches.contains(&key.as_str()) {
            flags.insert(key);
            i += 1;
        } else {
            return Err(usage(format!("unknown option '{a}'")));
        }
    }
    let quiet = flags.contains("-q");
    let say = |m: String| {
        if !quiet {
            println!("{m}");
        }
    };
    let num = |k: &str| -> Result<Option<f32>, Fail> { opts.get(k).map(|v| v.parse::<f32>().map_err(|_| usage(format!("{k}: '{v}' is not a number")))).transpose() };

    let path = std::path::PathBuf::from(scene_path);
    if !path.is_file() {
        return Err((1, format!("scene not found: {}", path.display())));
    }
    let mut app = App::new();
    app.open_from(path.clone());
    if app.file_path.as_ref() != Some(&path) {
        return Err((1, app.cmd_result.clone()));
    }
    if let Some(m) = opts.get("-mel") {
        crate::commands::execute(&mut app, m).map_err(|e| (1, format!("MEL: {e}")))?;
    }

    // Settings: saved ones, then MEL-style overrides through renderSettings (same parsing).
    let mut mel = String::from("renderSettings");
    for k in ["-x", "-y", "-spp", "-b", "-env", "-exp", "-tm", "-dn", "-bg", "-mb", "-shutter"] {
        if let Some(v) = opts.get(k) {
            mel.push_str(&format!(" {k} {v}"));
        }
    }
    crate::commands::execute(&mut app, &mel).map_err(|e| usage(e.to_string()))?;
    let settings = app.render.settings.clone();

    // Camera.
    let cam_name: Option<String> = match opts.get("-cam").or(opts.get("-camera")) {
        Some(c) => {
            let id = app.scene.find_by_name(c).ok_or_else(|| (1, format!("no camera named '{c}'")))?;
            if !app.is_camera(id) {
                return Err((1, format!("'{c}' is not a camera")));
            }
            Some(c.clone())
        }
        None => match app.render.camera {
            RenderCam::Node(id) => app.scene.node(id).map(|n| n.name.clone()),
            RenderCam::ActiveView => {
                let ids: Vec<_> = app.scene.nodes.iter().map(|n| n.id).collect();
                ids.into_iter().find(|&id| app.is_camera(id)).and_then(|id| app.scene.node(id).map(|n| n.name.clone()))
            }
        },
    };
    if cam_name.is_none() {
        say("warning: the scene has no camera; rendering from the default perspective view".into());
    }

    // Frames.
    let (start, end, seq) = if flags.contains("-all") {
        (app.scene.play_start.round() as i32, app.scene.play_end.round() as i32, true)
    } else if let Some(s) = num("-s")? {
        let e = num("-e")?.unwrap_or(s);
        (s.round() as i32, e.round() as i32, true)
    } else if num("-e")?.is_some() {
        return Err(usage("-e needs -s"));
    } else {
        let f = num("-f")?.unwrap_or(app.scene.time).round() as i32;
        (f, f, false)
    };
    if end < start {
        return Err(usage(format!("end frame {end} is before start frame {start}")));
    }
    let out = opts.get("-o").cloned().unwrap_or_else(|| path.with_extension("png").to_string_lossy().into_owned());
    let passes = flags.contains("-passes");
    let mattes = opts.get("-mattes").cloned();
    let resume = flags.contains("-resume");
    let step = match num("-step")? {
        Some(v) if v >= 1.0 => v as i32,
        Some(_) => return Err(usage("-step must be 1 or more")),
        None => 1,
    };
    let mut frames: Vec<i32> = (start..=end).step_by(step as usize).collect();
    let mut part = String::new();
    if let Some(c) = opts.get("-chunk") {
        let (i, n) = c.split_once('/').and_then(|(a, b)| Some((a.trim().parse::<usize>().ok()?, b.trim().parse::<usize>().ok()?))).filter(|&(i, n)| n >= 1 && (1..=n).contains(&i)).ok_or_else(|| usage(format!("-chunk '{c}': expected i/n with 1 ≤ i ≤ n, e.g. 2/4")))?;
        let len = frames.len();
        frames = frames[len * (i - 1) / n..len * i / n].to_vec();
        part = format!(" (chunk {i}/{n})");
    }
    let seq = seq || step > 1 || opts.contains_key("-chunk") || resume;
    if frames.is_empty() {
        say(format!("Nothing to render{part}: the chunk is empty."));
        return Ok(());
    }
    let (start, end) = (frames[0], *frames.last().unwrap());

    say(format!(
        "Rendering {} — {}x{}, {} spp, {} bounces{}, camera {}, frame{} {}{part}",
        path.display(),
        settings.width,
        settings.height,
        settings.spp,
        settings.bounces,
        if settings.motion_blur { format!(", motion blur {} fr", settings.shutter) } else { String::new() },
        cam_name.as_deref().unwrap_or("persp (default)"),
        if frames.len() == 1 { "" } else { "s" },
        if frames.len() == 1 { start.to_string() } else if step > 1 { format!("{start}-{end} every {step}") } else { format!("{start}-{end}") },
    ));
    let t0 = std::time::Instant::now();
    let total = frames.len();
    let mut rendered = total;
    if seq {
        let (mut done, mut skipped, mut timed) = (0usize, 0usize, 0usize);
        let fo = crate::render_view::FrameOutput { passes, mattes, skip_existing: resume };
        app.render_frames(cam_name.as_deref(), settings, &frames, &out, &fo, &mut |f, secs, files| {
            done += 1;
            if secs < 0.0 {
                skipped += 1;
                if !quiet {
                    println!("[{done}/{total}] frame {f}: already done, skipped");
                }
                return;
            }
            timed += 1;
            if !quiet {
                // ETA from the frames actually rendered this run.
                let el = t0.elapsed().as_secs_f32();
                let eta = el / timed as f32 * (total - done) as f32;
                println!("[{done}/{total}] frame {f}: {secs:.1}s → {}{}", files.first().map(String::as_str).unwrap_or(""), if done < total { format!("  (ETA {})", fmt_secs(eta)) } else { String::new() });
            }
        })
        .map_err(|e| (1, e))?;
        rendered = total - skipped;
    } else {
        app.set_time(start as f32);
        let mut job = app.render_now(cam_name.as_deref(), settings.clone()).map_err(|e| (1, e))?;
        let mut files = if passes {
            job.save_passes(std::path::Path::new(&out), settings.exposure, settings.tonemap).map_err(|e| (1, e))?
        } else {
            job.save_png(std::path::Path::new(&out), settings.exposure, settings.tonemap).map_err(|e| (1, e))?;
            vec![out.clone()]
        };
        if let Some(spec) = &mattes {
            let ext = std::path::Path::new(&out).extension().and_then(|e| e.to_str()).unwrap_or("png").to_string();
            files.extend(app.save_mattes(&job, spec, crate::render_view::base_without_ext(&out), None, &ext).map_err(|e| (1, e))?);
        }
        say(format!("frame {start}: {:.1}s → {}", job.elapsed().as_secs_f32(), files.join(", ")));
    }
    let skipped = if rendered < total { format!(", {} already done", total - rendered) } else { String::new() };
    say(format!("Done: {rendered} frame{} in {}{skipped}{}", if rendered == 1 { "" } else { "s" }, fmt_secs(t0.elapsed().as_secs_f32()), if seq { format!(" ({} …)", frame_path(&out, start)) } else { String::new() }));
    Ok(())
}

fn fmt_secs(s: f32) -> String {
    if s < 60.0 {
        format!("{s:.1}s")
    } else if s < 3600.0 {
        format!("{}m{:02}s", (s / 60.0) as u32, (s % 60.0) as u32)
    } else {
        format!("{}h{:02}m", (s / 3600.0) as u32, ((s % 3600.0) / 60.0) as u32)
    }
}

/// On Windows a GUI-subsystem exe has no console; attach to the one it was started from so
/// batch output shows up. When output is already redirected (a pipe or file, as in CI or
/// `| Out-Host`), the inherited handles are kept.
#[cfg(windows)]
pub fn attach_console() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn AttachConsole(pid: u32) -> i32;
        fn GetStdHandle(which: u32) -> *mut core::ffi::c_void;
    }
    // STD_OUTPUT_HANDLE = (DWORD)-11; ATTACH_PARENT_PROCESS = (DWORD)-1.
    unsafe {
        let h = GetStdHandle(-11i32 as u32);
        if h.is_null() || h as isize == -1 {
            AttachConsole(u32::MAX);
        }
    }
}

#[cfg(not(windows))]
pub fn attach_console() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn batch_renders_saved_scene() {
        let dir = std::env::temp_dir().join(format!("rm_batch_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut app = App::new();
        crate::commands::execute(&mut app, "polySphere -n ball; setKeyframe -at tx -t 1 -v -2 ball; setKeyframe -at tx -t 3 -v 2 ball; camera -n shotCam; move 0 0 8 shotCam; renderSettings -x 48 -y 32 -spp 2 -cam shotCam").unwrap();
        let scene = dir.join("shot.rma");
        app.save_to(scene.clone());
        let sc = scene.to_string_lossy().into_owned();
        let out = dir.join("out/shot.png").to_string_lossy().into_owned();
        assert_eq!(run(&s(&[&sc, "-s", "1", "-e", "3", "-o", &out, "-q"])), 0);
        for f in 1..=3 {
            let p = frame_path(&out, f);
            let img = image::open(&p).unwrap_or_else(|e| panic!("{p}: {e}"));
            assert_eq!((img.width(), img.height()), (48, 32));
        }
        // Single frame with passes as EXR, motion blur on.
        let exr = dir.join("out/f2.exr").to_string_lossy().into_owned();
        assert_eq!(run(&s(&[&sc, "-f", "2", "-mb", "on", "-passes", "-o", &exr, "-q"])), 0);
        for tag in ["beauty", "depth", "diffuse_direct", "specular"] {
            let p = dir.join(format!("out/f2_{tag}.exr"));
            let img = image::open(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            assert!(matches!(img, image::DynamicImage::ImageRgba32F(_)));
        }
        // Errors.
        assert_eq!(run(&s(&[&sc, "-bogus"])), 2);
        assert_eq!(run(&s(&[&sc, "-cam", "nope", "-q"])), 1);
        assert_eq!(run(&s(&["missing.rma"])), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn batch_chunks_steps_resumes_and_mattes() {
        let dir = std::env::temp_dir().join(format!("rm_batch2_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut app = App::new();
        crate::commands::execute(&mut app, "polySphere -n ball; polyCube -n crate; move 2 0 0 crate; camera -n cam -p 0 1 8 -aim 0 0 0; renderSettings -x 24 -y 16 -spp 1 -cam cam").unwrap();
        let scene = dir.join("s.rma");
        app.save_to(scene.clone());
        let sc = scene.to_string_lossy().into_owned();
        let out = dir.join("r/f.png").to_string_lossy().into_owned();
        let exists = |f: i32| std::path::Path::new(&frame_path(&out, f)).exists();
        // Frames 1..=10 split in 3: [1,2,3] [4,5,6] [7..10].
        assert_eq!(run(&s(&[&sc, "-s", "1", "-e", "10", "-chunk", "2/3", "-o", &out, "-q"])), 0);
        assert_eq!((1..=10).filter(|&f| exists(f)).collect::<Vec<_>>(), vec![4, 5, 6]);
        // Every 4th frame of 1..=10: 1, 5 (exists), 9.
        let before = std::fs::metadata(frame_path(&out, 5)).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(run(&s(&[&sc, "-s", "1", "-e", "10", "-step", "4", "-resume", "-o", &out, "-q"])), 0);
        assert_eq!((1..=10).filter(|&f| exists(f)).collect::<Vec<_>>(), vec![1, 4, 5, 6, 9]);
        assert_eq!(std::fs::metadata(frame_path(&out, 5)).unwrap().modified().unwrap(), before, "resume skipped frame 5");
        // No temporary files left behind.
        let names: Vec<String> = std::fs::read_dir(dir.join("r")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert!(names.iter().all(|n| !n.contains("partial")), "{names:?}");
        // Mattes per frame, and bad chunk specs.
        assert_eq!(run(&s(&[&sc, "-s", "2", "-e", "3", "-mattes", "all", "-o", &out, "-q"])), 0);
        assert!(dir.join("r/f_matte_ball.0002.png").exists() && dir.join("r/f_matte_crate.0003.png").exists());
        assert_eq!(run(&s(&[&sc, "-mattes", "ghost", "-o", &out, "-q"])), 1);
        for bad in ["0/3", "4/3", "x", "2/0"] {
            assert_eq!(run(&s(&[&sc, "-s", "1", "-e", "3", "-chunk", bad, "-q"])), 2, "{bad}");
        }
        // A chunk can be empty when there are fewer frames than machines.
        assert_eq!(run(&s(&[&sc, "-s", "1", "-e", "2", "-chunk", "1/3", "-o", &out, "-q"])), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
