//! `cargo xtask package 3ds --cia` (issue #61): the 3DS build as an
//! installable `.cia` (real hardware with a homebrew title manager; the
//! `.3dsx` stays what the Homebrew Launcher and Azahar run).
//!
//! It runs `build 3ds`, then `bannertool makebanner` (the HOME Menu banner:
//! `[3ds] banner` (256 × 128 PNG) and `banner_audio` (a WAV of at most
//! 3 s), else the icon on a dark band and a silent jingle) and `makerom -f
//! cia` with a generated RSF (unique id and product code from `[3ds]
//! unique_id` / `product_code`, else derived from the project name; the
//! New 3DS's 804 MHz and L2 cache asked for). `ctrtool` reads the result
//! back when it is there. `--selftest` packs the self-test build; `--no-build`
//! packs what `build 3ds` left (the ELF, the RomFS, the SMDH). The tools come from `$TRINO_MAKEROM` /
//! `$TRINO_BANNERTOOL` / `$TRINO_CTRTOOL`, `PATH`, or are fetched on Linux
//! x86_64 into `target/tools` from pinned releases (checksums below).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;

use crate::project::Project;

struct Tool {
    name: &'static str,
    env: &'static str,
    url: &'static str,
    sha256: &'static str,
    /// The executable inside the zip.
    inner: &'static str,
}

const MAKEROM: Tool = Tool {
    name: "makerom",
    env: "TRINO_MAKEROM",
    url: "https://github.com/3DSGuy/Project_CTR/releases/download/makerom-v0.19.0/makerom-v0.19.0-ubuntu_x86_64.zip",
    sha256: "287b809dec064e0ad597e3d272c49ecb7eed41693d5ee6fef9d8a8aa24c2497e",
    inner: "makerom",
};
const BANNERTOOL: Tool = Tool {
    name: "bannertool",
    env: "TRINO_BANNERTOOL",
    url: "https://github.com/diasurgical/bannertool/releases/download/1.2.0/bannertool.zip",
    sha256: "69768596f836acb3e3aeaa66e47c6ba560dde813c6dfcd33c8afc25fe29b7524",
    inner: "linux-x86_64/bannertool",
};
const CTRTOOL: Tool = Tool {
    name: "ctrtool",
    env: "TRINO_CTRTOOL",
    url: "https://github.com/3DSGuy/Project_CTR/releases/download/ctrtool-v1.3.0/ctrtool-v1.3.0-ubuntu_x86_64.zip",
    sha256: "f553194b0ab2b539457160231cf5651c554e148f17ec960fae63424e82582aa8",
    inner: "ctrtool",
};

fn on_path(name: &str) -> bool {
    Command::new(name).arg("--help").output().is_ok()
}

/// The tool: its env var, `PATH`, `target/tools`, else fetched (Linux
/// x86_64) and checked.
fn tool(engine: &Path, t: &Tool) -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var(t.env) {
        return Ok(PathBuf::from(p));
    }
    if on_path(t.name) {
        return Ok(PathBuf::from(t.name));
    }
    let dir = engine.join("target/tools");
    let exe = dir.join(t.name);
    if exe.is_file() {
        return Ok(exe);
    }
    if !(cfg!(target_os = "linux") && cfg!(target_arch = "x86_64")) {
        return Err(format!(
            "{}: not found; install it on PATH or set {} ({})",
            t.name, t.env, t.url
        ));
    }
    let zip = dir.join(format!("{}.zip", t.name));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let ok = Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(&zip)
        .arg(t.url)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        return Err(format!("download {} failed (curl)", t.url));
    }
    let sum = Command::new("sha256sum")
        .arg(&zip)
        .output()
        .map_err(|e| format!("sha256sum: {e}"))?;
    if !String::from_utf8_lossy(&sum.stdout).starts_with(t.sha256) {
        let _ = std::fs::remove_file(&zip);
        return Err(format!("{}: checksum mismatch", t.url));
    }
    let unpack = dir.join(format!("{}-unpacked", t.name));
    let ok = Command::new("unzip")
        .args(["-o", "-q"])
        .arg(&zip)
        .arg("-d")
        .arg(&unpack)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        return Err(format!("unzip {} failed", zip.display()));
    }
    std::fs::copy(unpack.join(t.inner), &exe).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755));
    }
    Ok(exe)
}

/// A homebrew unique id (`0xF8000`–`0xFFFFF`) from the project name.
pub fn unique_id(project: &Project) -> String {
    project.cia.unique_id.clone().unwrap_or_else(|| {
        format!(
            "0x{:05X}",
            0xF8000 + trino_core::asset_id(&project.name) % 0x7FFF
        )
    })
}

/// `CTR-P-XXXX` from the project name (letters and digits, padded with T).
pub fn product_code(project: &Project) -> String {
    project.cia.product_code.clone().unwrap_or_else(|| {
        let mut s: String = project
            .name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_uppercase())
            .take(4)
            .collect();
        while s.len() < 4 {
            s.push('T');
        }
        format!("CTR-P-{s}")
    })
}

/// The RSF makerom reads: a homebrew application on the SD card, the New
/// 3DS's clock and cache on, the services the shim uses.
pub fn rsf(project: &Project, romfs: &Path) -> String {
    let title: String = project
        .title
        .chars()
        .filter(|c| *c != '"')
        .take(64)
        .collect();
    format!(
        r#"BasicInfo:
  Title                   : "{title}"
  ProductCode             : "{code}"
  Logo                    : Homebrew

RomFs:
  RootPath                : "{romfs}"

TitleInfo:
  Category                : Application
  UniqueId                : {uid}

Option:
  UseOnSD                 : true
  FreeProductCode         : true
  MediaFootPadding        : false
  EnableCrypt             : false
  EnableCompress          : true

AccessControlInfo:
  CoreVersion                   : 2
  DescVersion                   : 2
  ReleaseKernelMajor            : "02"
  ReleaseKernelMinor            : "33"
  UseExtSaveData                : false
  FileSystemAccess:
   - DirectSdmc
   - DirectSdmcWrite
  MemoryType                    : Application
  SystemMode                    : 64MB
  IdealProcessor                : 0
  AffinityMask                  : 1
  Priority                      : 16
  MaxCpu                        : 0x9E
  HandleTableSize               : 0x200
  DisableDebug                  : false
  EnableForceDebug              : false
  CanWriteSharedPage            : true
  CanUsePrivilegedPriority      : false
  CanUseNonAlphabetAndNumber    : true
  PermitMainFunctionArgument    : true
  CanShareDeviceMemory          : true
  RunnableOnSleep               : false
  SpecialMemoryArrange          : true
  SystemModeExt                 : Legacy
  CpuSpeed                      : 804MHz
  EnableL2Cache                 : true
  CanAccessCore2                : true
  IORegisterMapping:
   - 1ff00000-1ff7ffff
  MemoryMapping:
   - 1f000000-1f5fffff:r
  SystemCallAccess:
    ControlMemory: 1
    QueryMemory: 2
    ExitProcess: 3
    GetProcessAffinityMask: 4
    SetProcessAffinityMask: 5
    GetProcessIdealProcessor: 6
    SetProcessIdealProcessor: 7
    CreateThread: 8
    ExitThread: 9
    SleepThread: 10
    GetThreadPriority: 11
    SetThreadPriority: 12
    GetThreadAffinityMask: 13
    SetThreadAffinityMask: 14
    GetThreadIdealProcessor: 15
    SetThreadIdealProcessor: 16
    GetCurrentProcessorNumber: 17
    Run: 18
    CreateMutex: 19
    ReleaseMutex: 20
    CreateSemaphore: 21
    ReleaseSemaphore: 22
    CreateEvent: 23
    SignalEvent: 24
    ClearEvent: 25
    CreateTimer: 26
    SetTimer: 27
    CancelTimer: 28
    ClearTimer: 29
    CreateMemoryBlock: 30
    MapMemoryBlock: 31
    UnmapMemoryBlock: 32
    CreateAddressArbiter: 33
    ArbitrateAddress: 34
    CloseHandle: 35
    WaitSynchronization1: 36
    WaitSynchronizationN: 37
    SignalAndWait: 38
    DuplicateHandle: 39
    GetSystemTick: 40
    GetHandleInfo: 41
    GetSystemInfo: 42
    GetProcessInfo: 43
    GetThreadInfo: 44
    ConnectToPort: 45
    SendSyncRequest1: 46
    SendSyncRequest2: 47
    SendSyncRequest3: 48
    SendSyncRequest4: 49
    SendSyncRequest: 50
    OpenProcess: 51
    OpenThread: 52
    GetProcessId: 53
    GetProcessIdOfThread: 54
    GetThreadId: 55
    GetResourceLimit: 56
    GetResourceLimitLimitValues: 57
    GetResourceLimitCurrentValues: 58
    GetThreadContext: 59
    Break: 60
    OutputDebugString: 61
    ControlPerformanceCounter: 62
    CreatePort: 71
    CreateSessionToPort: 72
    CreateSession: 73
    AcceptSession: 74
    ReplyAndReceive1: 75
    ReplyAndReceive2: 76
    ReplyAndReceive3: 77
    ReplyAndReceive4: 78
    ReplyAndReceive: 79
    BindInterrupt: 80
    UnbindInterrupt: 81
    InvalidateProcessDataCache: 82
    StoreProcessDataCache: 83
    FlushProcessDataCache: 84
    StartInterProcessDma: 85
    StopDma: 86
    GetDmaState: 87
    RestartDma: 88
    DebugActiveProcess: 96
    BreakDebugProcess: 97
    TerminateDebugProcess: 98
    GetProcessDebugEvent: 99
    ContinueDebugEvent: 100
    GetProcessList: 101
    GetThreadList: 102
    GetDebugThreadContext: 103
    SetDebugThreadContext: 104
    QueryDebugProcessMemory: 105
    ReadProcessMemory: 106
    WriteProcessMemory: 107
    SetHardwareBreakPoint: 108
    GetDebugThreadParam: 109
    ControlProcessMemory: 112
    MapProcessMemory: 113
    UnmapProcessMemory: 114
    CreateCodeSet: 115
    CreateProcess: 117
    TerminateProcess: 118
    SetProcessResourceLimits: 119
    CreateResourceLimit: 120
    SetResourceLimitValues: 121
    AddCodeSegment: 122
    Backdoor: 123
    KernelSetState: 124
    QueryProcessMemory: 125
  InterruptNumbers:
  ServiceAccessControl:
   - APT:U
   - $hioFIO
   - $hostio0
   - $hostio1
   - ac:u
   - boss:U
   - cam:u
   - cecd:u
   - cfg:nor
   - cfg:u
   - csnd:SND
   - dsp::DSP
   - frd:u
   - fs:USER
   - gsp::Gpu
   - gsp::Lcd
   - hid:USER
   - http:C
   - ir:rst
   - ir:u
   - ir:USER
   - mic:u
   - ndm:u
   - news:s
   - nwm::EXT
   - nwm::UDS
   - ptm:sysm
   - ptm:u
   - pxi:dev
   - soc:U
   - ssl:C
   - y2r:u

SystemControlInfo:
  SaveDataSize: 0KB
  RemasterVersion: 0
  StackSize: 0x40000
  Dependency:
    ac: 0x0004013000002402
    act: 0x0004013000003802
    am: 0x0004013000001502
    boss: 0x0004013000003402
    camera: 0x0004013000001602
    cecd: 0x0004013000002602
    cfg: 0x0004013000001702
    codec: 0x0004013000001802
    csnd: 0x0004013000002702
    dlp: 0x0004013000002802
    dsp: 0x0004013000001a02
    friends: 0x0004013000003202
    gpio: 0x0004013000001b02
    gsp: 0x0004013000001c02
    hid: 0x0004013000001d02
    http: 0x0004013000002902
    i2c: 0x0004013000001e02
    ir: 0x0004013000003302
    mcu: 0x0004013000001f02
    mic: 0x0004013000002002
    ndm: 0x0004013000002b02
    news: 0x0004013000003502
    nim: 0x0004013000002c02
    nwm: 0x0004013000002d02
    pdn: 0x0004013000002102
    ps: 0x0004013000003102
    ptm: 0x0004013000002202
    ro: 0x0004013000003702
    socket: 0x0004013000002e02
    spi: 0x0004013000002302
    ssl: 0x0004013000002f02
"#,
        code = product_code(project),
        uid = unique_id(project),
        romfs = romfs.display(),
    )
}

/// A 256 × 128 banner from the icon: centered at 96 px on a dark band.
fn default_banner(project: &Project, out: &Path) -> Result<(), String> {
    let icon = match &project.n3ds_icon {
        Some(p) => crate::package::icon::Image::load(p)?,
        None => crate::package::icon::default_icon(),
    }
    .resized(96);
    let (w, h) = (256u32, 128u32);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let (ix, iy) = (x as i32 - 80, y as i32 - 16);
            let mut c = [28u8, 30, 44, 255];
            if (0..96).contains(&ix) && (0..96).contains(&iy) {
                let i = ((iy as u32 * 96 + ix as u32) * 4) as usize;
                let a = icon.rgba[i + 3] as u32;
                for (k, ch) in c.iter_mut().take(3).enumerate() {
                    *ch = ((icon.rgba[i + k] as u32 * a + *ch as u32 * (255 - a)) / 255) as u8;
                }
            }
            rgba.extend_from_slice(&c);
        }
    }
    let img = crate::package::icon::Image { w, h, rgba };
    std::fs::write(out, img.png()).map_err(|e| e.to_string())
}

/// One second of silence (22 050 Hz mono, 16-bit): the banner's jingle.
fn silent_wav(out: &Path) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 22_050,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(out, spec).map_err(|e| e.to_string())?;
    for _ in 0..22_050 {
        w.write_sample(0i16).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())
}

fn run(cmd: &mut Command, what: &str) -> Result<String, String> {
    let out = cmd.output().map_err(|e| format!("{what}: {e}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if !out.status.success() {
        return Err(format!("{what} failed:\n{text}"));
    }
    Ok(text)
}

pub fn cia(
    project: &Project,
    out: Option<PathBuf>,
    selftest: bool,
    no_build: bool,
    json: bool,
) -> Result<serde_json::Value, String> {
    let makerom = tool(&project.engine, &MAKEROM)?;
    let bannertool = tool(&project.engine, &BANNERTOOL)?;
    // `--selftest`: the self-test build (`build 3ds --selftest`), to boot
    // the installed title in an emulator. `--no-build`: what `build 3ds`
    // left (the ELF, the RomFS, the SMDH) — CI builds in the devkitARM
    // container, whose glibc (2.36) is older than makerom's (2.38), and
    // packs on Ubuntu.
    if no_build {
        let smdh = project
            .root
            .join(format!("target/3ds/{}.smdh", project.name));
        for need in [
            project.n3ds_elf(),
            project.root.join("target/3ds/romfs"),
            smdh,
        ] {
            if !need.exists() {
                return Err(format!(
                    "package 3ds --cia --no-build: {} missing (cargo xtask build 3ds first)",
                    need.display()
                ));
            }
        }
    } else {
        crate::n3ds::build_steps(project, selftest)?;
    }
    let work = project.root.join("target/3ds/cia");
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let banner_png = match &project.cia.banner {
        Some(p) => p.clone(),
        None => {
            let p = work.join("banner.png");
            default_banner(project, &p)?;
            p
        }
    };
    let audio = match &project.cia.banner_audio {
        Some(p) => p.clone(),
        None => {
            let p = work.join("banner.wav");
            silent_wav(&p)?;
            p
        }
    };
    let bnr = work.join("banner.bnr");
    run(
        Command::new(&bannertool)
            .args(["makebanner", "-i"])
            .arg(&banner_png)
            .arg("-a")
            .arg(&audio)
            .arg("-o")
            .arg(&bnr),
        "bannertool makebanner",
    )?;
    let rsf_path = work.join("app.rsf");
    std::fs::write(
        &rsf_path,
        rsf(project, &project.root.join("target/3ds/romfs")),
    )
    .map_err(|e| e.to_string())?;
    let out_dir = out.unwrap_or_else(|| project.root.join("dist"));
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let cia = out_dir.join(format!("{}.cia", project.name));
    let smdh = project
        .root
        .join(format!("target/3ds/{}.smdh", project.name));
    run(
        Command::new(&makerom)
            .args(["-f", "cia", "-o"])
            .arg(&cia)
            .args(["-target", "t", "-exefslogo", "-elf"])
            .arg(project.n3ds_elf())
            .arg("-rsf")
            .arg(&rsf_path)
            .arg("-icon")
            .arg(&smdh)
            .arg("-banner")
            .arg(&bnr),
        "makerom",
    )?;
    // The same NCCH as a .cxi: what an emulator boots directly
    // (`TRINO_3DS_APP=<file> cargo xtask test 3ds --no-build`).
    let cxi = out_dir.join(format!("{}.cxi", project.name));
    run(
        Command::new(&makerom)
            .args(["-f", "cxi", "-o"])
            .arg(&cxi)
            .args(["-target", "t", "-exefslogo", "-elf"])
            .arg(project.n3ds_elf())
            .arg("-rsf")
            .arg(&rsf_path)
            .arg("-icon")
            .arg(&smdh)
            .arg("-banner")
            .arg(&bnr),
        "makerom (cxi)",
    )?;
    let size = std::fs::metadata(&cia).map_or(0, |m| m.len());
    let verified = match tool(&project.engine, &CTRTOOL) {
        Ok(ctrtool) => {
            let text = run(Command::new(ctrtool).arg("-i").arg(&cia), "ctrtool")?;
            let code = product_code(project);
            if !text.contains(&code) {
                return Err(format!(
                    "ctrtool does not read {code} back from {}",
                    cia.display()
                ));
            }
            Some(code)
        }
        Err(_) => None,
    };
    let line = format!(
        "[3ds] {} ({:.1} MB, {}, unique id {}){}",
        cia.display(),
        size as f64 / 1e6,
        product_code(project),
        unique_id(project),
        if verified.is_some() {
            ", read back by ctrtool"
        } else {
            ""
        }
    );
    if json {
        eprintln!("{line}");
    } else {
        println!("{line}");
    }
    Ok(json!({
        "cia": cia,
        "cxi": cxi,
        "bytes": size,
        "product_code": product_code(project),
        "unique_id": unique_id(project),
        "verified_by_ctrtool": verified.is_some(),
    }))
}
