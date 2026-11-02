# Smart disk I/O (proposed D48)

Written 2026-11-02_110639_1793613999. A proposal, not approved; no code before the owner answers the questions at the end. The owner's idea: "Smart recheck. Throttle write and/or read speeds upon high disk IO detection. Same for download speed/upload speed. We could also check if the storage is SSD, HDD or NAS." It goes beyond 2.2.1, so it gets its own Preferences page.

## What the engine does today

- **Checking** runs on one thread, `lt-hash` (`DiskIo::async_check`, `check_files` in `crates/core/src/disk/mod.rs`). Checks run one after another for every drive, whole pieces at a time, as fast as the disk allows, at normal I/O priority. `active_checking` exists in `Settings` but nothing reads it; the single thread behaves as `active_checking = 1`. A check on a slow NAS share holds up a check on a fast SSD queued behind it.
- **Reads and writes** go to `aio_threads` threads (default 10) by `piece % threads`, so every thread can touch every drive. Ten threads seeking on one HDD at once is the classic thrash.
- **Write back-pressure:** the engine stops reading from peers while the queued writes exceed max(`max_queued_disk_bytes`, 4 MiB) (`disk_backlogged`, `crates/core/src/engine/mod.rs`). So downloads already slow down to what the disk takes; there is no headroom left for other programs.
- **Reads for peers** go through the read cache (Disk Cache page); nothing slows uploads for the disk.
- Nothing detects the drive type, sets an I/O priority or measures disk latency.

## Proposal

Four parts, each usable alone and approvable separately, cheapest and most useful first. All of it lives in `core` with settings in `Settings`, so nox, the Web UI and both GUIs get it, and the page is declared once in `frontend::pref_pages`. Nothing changes on the wire: `PEER_ID_PREFIX` stays; the remote protocol moves to 12 for the new settings. No new dependencies: Windows calls through the `windows-sys` features core already has (`Win32_System_Ioctl`, `Win32_Storage_FileSystem`, `Win32_System_Threading`), Unix calls through `extern "C"` blocks as `net/sys.rs` does.

### 1. Rechecks at low disk priority, with an optional cap

- The checking thread runs at background I/O priority:
  - **Windows:** `SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN)` on `lt-hash`. Its I/O and memory priority drop to very low; another program's disk use wins at once.
  - **Linux:** `ioprio_set(IOPRIO_WHO_PROCESS, tid, IOPRIO_CLASS_IDLE)`. BFQ honours it, `mq-deadline` only partly (priority classes since Linux 5.14), and `none` (the usual NVMe default) not at all, so on Linux the cap below matters more.
  - **macOS:** `setiopolicy_np(IOPOL_TYPE_DISK, IOPOL_SCOPE_THREAD, IOPOL_THROTTLE)`.
  - **FreeBSD:** no per-thread I/O priority; the cap only.
  - A NAS never sees any of these: the server schedules its own disks.
- **Cap:** "Limit checking to N MB/s" (off by default), a token bucket in the `check_files` loop. Someone asked qBittorrent for exactly this (issue 17632: a recheck made a SATA SSD lag; they wanted about 100 MB/s).
- **Cost:** a background check can take much longer while something else uses the disk; that is the point. A check with resume data (`CheckMode::Resume`) reads no data and is unaffected.

### 2. One check per drive, drives in parallel

- The check queue is keyed by drive: the physical disk where known, else the volume, else the server of a share (part 3 finds them). One check runs per drive; checks on different drives run at once, up to `active_checking` (finally read; proposed default 4, since different drives no longer compete).
- `lt-hash` becomes a small pool of `active_checking` checking threads; fences and barriers (stop, remove, move, file priority) go to each of them, as they go to `lt-hash` today.
- Partitions of one disk are one drive: Windows `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS`; Linux the partition's parent in `/sys/dev/block/<major>:<minor>`. Where it cannot be told (ZFS, `mergerfs`, Storage Spaces), the volume is the key.
- The drive is looked up when a torrent's storage is created and kept in a small map by save path; nothing is added to the compact stopped torrent (the D21 size test stays at 24 bytes).

### 3. Drive type detection

Each save path's drive is classed **SSD, HDD, Network, Removable or Unknown**:

| Platform | Network | Removable | SSD / HDD |
|---|---|---|---|
| Windows | UNC path, or `GetDriveTypeW` = `DRIVE_REMOTE` (mapped drive) | `DRIVE_REMOVABLE`, or bus type USB | `IOCTL_STORAGE_QUERY_PROPERTY`: `StorageDeviceSeekPenaltyProperty` (`IncursSeekPenalty`) and `StorageDeviceProperty` (bus type: NVMe, SATA, USB, iSCSI, Spaces, Virtual) on `\\.\PhysicalDriveN`, opened with no access rights (to verify: works without elevation) |
| Linux | `statfs` `f_type`: NFS, CIFS / SMB2, FUSE (`sshfs`, `rclone`) | `/sys/block/<disk>/removable` | `/sys/block/<disk>/queue/rotational`, through `dm-*` / `md*` `slaves/` to the real disks |
| macOS | `statfs` `f_fstypename`: `smbfs`, `nfs`, `afpfs`, `webdav` | IOKit `Removable` / `Ejectable` | IOKit `Device Characteristics` > `Medium Type` (Solid State / Rotational) |
| FreeBSD | `statfs` `f_fstypename`: `nfs`, `smbfs`, `fusefs` | `da*` on `umass` | GEOM attribute `GEOM::rotation_rate` (`DIOCGATTR`; 1 = non-rotating) |

- **Wrong answers are normal:** USB bridges, RAID controllers and VMs often report the wrong type (the Xubuntu VM's virtio disk says rotational), a NAS share says nothing about its disks, ZFS pools and `mergerfs` have no single answer. So the type only chooses *defaults*; it is never trusted for anything that could lose data, and the Logger names it so a wrong guess is visible ("Downloads on D: (HDD, SATA)").
- **What the type changes** (only when "Tune for the drive type" is on):
  - **HDD and Removable:** at most 2 disk jobs in flight on that drive (a per-drive semaphore in front of the `aio_threads`; reads and writes keep their threads), whole-piece read cache lines (they won on the Windows test disk, BACKLOG "Disk and storage"), one check at a time (part 2).
  - **SSD:** as today; two checks may share an NVMe drive.
  - **Network:** at most 4 jobs in flight per server and one check per server; the busy-disk throttle (part 4) uses a larger delay allowance.
  - **Unknown:** as today.

### 4. Slowing down when the disk is busy

The existing back-pressure lets PaTorrent use all of a disk. This part leaves room for other programs: it lowers PaTorrent's own effective limits when the disk is slow to answer, the way uTP (LEDBAT) treats network delay.

- **Signal:** the disk threads time every write job and every file read, per drive. A low percentile over the last minutes is the drive's baseline; the last two seconds' 90th percentile is the current delay. Our own timings work on every platform and include a NAS; they cannot tell another program's load from our own, but both mean the disk is busy.
- **Control:** per drive, when the current delay exceeds the baseline by more than the allowance (proposed: SSD 10 ms, HDD 50 ms, Network 100 ms, Unknown 50 ms), the drive's factor halves; while it is under, the factor grows by 5% a second. The engine applies the lowest factor among the drives a direction uses to the measured throughput: downloads stop being read from peers above it (the same place as `disk_backlogged`), uploads are deferred above it. It never raises a limit the user set.
- **Optional, Windows and Linux:** the OS's own busy counters (Windows `IOCTL_DISK_PERFORMANCE` per physical disk; Linux `/proc/diskstats` `io_ticks` and sectors), compared with our own bytes, to slow down only when *another* program keeps the disk busy (question Q4).
- **Visibility:** when the throttle is active, the status bar's speed tooltip and the Web UI say "Slowed for the disk (D:)", and the Logger records each start and end (at most one line a minute per drive), so nobody thinks the client is broken.
- **Risks:** oscillation (tested with a simulated disk delay in `MockConfig`, and on the VM with the throttled loop mount `tools/closing-test` already uses); a seed on a busy disk uploads less, which matters on ratio trackers (question Q5).

### Preferences: Advanced > Disk I/O

A new page beside Disk Cache, in all three GUIs and the Web UI (proposed texts):

```
Checking
  [x] Check torrents at low disk priority
  [ ] Limit checking to [ 100 ] MB/s
  Check up to [ 4 ] drives at once (one torrent per drive)
Drives
  [x] Tune for the drive type (SSD, hard disk, network)
When the disk is busy
  [ ] Slow downloads down (disk writes)
  [ ] Slow uploads down (disk reads)
```

Proposed defaults: low-priority checks and drive tuning on, the busy-disk throttle off until it has been measured. The detected drives are listed in the Logger at start; per-drive overrides ("treat E: as a hard disk") stay out of the lean UI unless the owner wants them (Q6), and would go in the Advanced table first.

## Not in this proposal

- The write cache D3 describes (still not implemented) and 2.2.1's "Disable Windows caching of disk writes/reads" boxes (unbuffered I/O); they are Disk Cache questions.
- Changing what a check reads: it already reads each piece once.
- CPU load or full-screen detection, unless the owner wants it (Q3).

## Order and size

1. Part 1: small, one patch version; immediately useful on Windows and macOS.
2. Part 2: medium (the check queue and its threads, drive keys).
3. Part 3: medium, mostly per-platform detection code and tests on the real machines (this PC, the test lab's Samba share, the Xubuntu VM, a USB disk).
4. Part 4: the largest; needs measurements before choosing its numbers.

Each part bumps the patch version (D37) when it lands. None touches the peer wire, so no interop rerun; parts 2-4 need checks on real disks, which go to BACKLOG "Disk and storage".

## Questions for the owner

- **Q1.** Approve the four parts, and the order 1, 2, 3, 4?
- **Q2.** Defaults: low-priority checks and drive tuning on, the busy-disk throttle off?
- **Q3.** Does "high load" mean only the disk, or also CPU load, or a full-screen game (Windows reports it: `SHQueryUserNotificationState` = `QUNS_RUNNING_D3D_FULL_SCREEN` / `QUNS_BUSY`)?
- **Q4.** Busy signal: our own timings only (every platform, includes NAS), or also the OS counters on Windows and Linux to react only to other programs?
- **Q5.** May the throttle slow uploads, given ratio trackers, or only downloads?
- **Q6.** Per-drive overrides in the UI, in the Advanced table only, or not at all?
- **Q7.** The page: a new "Disk I/O" under Advanced (proposed), or the checking options on Disk Cache?
- **Q8.** Does "smart recheck" also mean checking less? Two candidates: "Recheck this file" in the Files tab (only the pieces of the chosen files, including the pieces they share with neighbours), and remembering each file's size and modification time in the resume data so a forced recheck can say which files changed since the last save.

## Sources

- [SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN](https://learn.microsoft.com/windows/win32/api/processthreadsapi/nf-processthreadsapi-setthreadpriority)
- [DEVICE_SEEK_PENALTY_DESCRIPTOR](https://learn.microsoft.com/windows/win32/api/winioctl/ns-winioctl-device_seek_penalty_descriptor)
- [ioprio_set(2)](https://man7.org/linux/man-pages/man2/ioprio_set.2.html)
- [qBittorrent issue 17632: limit disk I/O while rechecking](https://github.com/qbittorrent/qBittorrent/issues/17632)
- [qBittorrent issue 11541: I/O errors on slow hard disks](https://github.com/qbittorrent/qBittorrent/issues/11541)
