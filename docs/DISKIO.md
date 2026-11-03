# Smart disk I/O (proposed D48)

Written 2026-11-02_110639_1793613999; the owner answered Q3 and Q8 on 2026-11-02_111330_1793614410. The owner answered the rest and approved it on 2026-11-03_084209_1793691729 (answers under the questions); built in 1.1.36-1.1.40, see "Implementation status" at the end. The owner's idea: "Smart recheck. Throttle write and/or read speeds upon high disk IO detection. Same for download speed/upload speed. We could also check if the storage is SSD, HDD or NAS." It goes beyond 2.2.1, so it gets its own Preferences page.

## What the engine does today

- **Checking** runs on one thread, `lt-hash` (`DiskIo::async_check`, `check_files` in `crates/core/src/disk/mod.rs`). Checks run one after another for every drive, whole pieces at a time, as fast as the disk allows, at normal I/O priority. `active_checking` exists in `Settings` but nothing reads it; the single thread behaves as `active_checking = 1`. A check on a slow NAS share holds up a check on a fast SSD queued behind it.
- **Reads and writes** go to `aio_threads` threads (default 10) by `piece % threads`, so every thread can touch every drive. Ten threads seeking on one HDD at once is the classic thrash.
- **Write back-pressure:** the engine stops reading from peers while the queued writes exceed max(`max_queued_disk_bytes`, 4 MiB) (`disk_backlogged`, `crates/core/src/engine/mod.rs`). So downloads already slow down to what the disk takes; there is no headroom left for other programs.
- **Reads for peers** go through the read cache (Disk Cache page); nothing slows uploads for the disk.
- Nothing detects the drive type, sets an I/O priority or measures disk latency.

## Proposal

Four parts, each usable alone and approvable separately, cheapest and most useful first. All of it lives in `core` with settings in `Settings`, so nox, the Web UI and both GUIs get it, and the page is declared once in `frontend::pref_pages`. Nothing changes on the wire: `PEER_ID_PREFIX` stays; the remote protocol moves to 12 for the new settings. No new dependencies: Windows calls through the `windows-sys` features core already has (`Win32_System_Ioctl`, `Win32_Storage_FileSystem`, `Win32_System_Threading`), Unix calls through `extern "C"` blocks as `net/sys.rs` does.

### 1. Gentle rechecks

The owner's meaning of "smart recheck" (2026-11-02_111330_1793614410): not checking less, but checking gently, with whatever each platform offers. So a check uses every tool below that its platform has; each is a no-op where it does not exist.

- **Low I/O priority** for the checking thread:
  - **Windows:** `SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN)` on `lt-hash`. Its I/O priority and its memory priority drop to very low; another program's disk use wins at once.
  - **Linux:** `ioprio_set(IOPRIO_WHO_PROCESS, tid, IOPRIO_CLASS_IDLE)`, the call `ionice -c 3` makes, for the one thread instead of the whole process. BFQ honours it, `mq-deadline` only partly (priority classes since Linux 5.14), and `none` (the usual NVMe default) not at all. A cgroup's `io.weight` (systemd `IOWeight=`) has the same scheduler limits and covers the whole daemon, so it is not used.
  - **macOS:** `setiopolicy_np(IOPOL_TYPE_DISK, IOPOL_SCOPE_THREAD, IOPOL_THROTTLE)`.
  - **FreeBSD:** no per-thread I/O priority.
  - A NAS never sees any of these: the server schedules its own disks.
- **Leave other programs' file cache alone.** A check of a 500 GB library otherwise pushes everything else out of the OS cache, and the whole machine feels slow afterwards.
  - **Windows:** the background mode above already puts the pages the check reads on the low-priority standby list, which is emptied first.
  - **Linux and FreeBSD:** `posix_fadvise(POSIX_FADV_SEQUENTIAL)` on the check's files (more read-ahead, fewer seeks), then `POSIX_FADV_DONTNEED` on each piece's range once it is hashed, unless the torrent is seeding from those pages.
  - **macOS:** `fcntl(F_NOCACHE)` and `F_RDAHEAD` on the check's file handles.
  - The check opens its own handles for this instead of the shared file pool's, so peers' reads keep the normal cache behaviour.
- **Yield on delay (every platform, including NVMe on Linux and a NAS):** the check times its own reads (the same measurement as part 4). When a piece takes clearly longer than the drive's recent baseline, someone else is using the disk: the check sleeps for as long as that read took before the next piece, so it uses at most about half of a busy disk, and runs at full speed on an idle one. This is the one tool that works where the OS priorities do nothing.
- **Cap:** "Limit checking to N MB/s" (off by default), a token bucket in the `check_files` loop. Someone asked qBittorrent for exactly this (issue 17632: a recheck made a SATA SSD lag; they wanted about 100 MB/s).
- **Cost:** a gentle check takes much longer while something else uses the disk; that is the point. A check with resume data (`CheckMode::Resume`) reads no data and is unaffected. CPU use is not limited (owner: CPU load is fine).

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
  [x] Check torrents gently (low priority, give way to other programs)
  [ ] Limit checking to [ 100 ] MB/s
  Check up to [ 4 ] drives at once (one torrent per drive)
Drives
  [x] Tune for the drive type (SSD, hard disk, network)
When the disk is busy
  [ ] Slow downloads down (disk writes)
  [ ] Slow uploads down (disk reads)
```

Proposed defaults: gentle checks and drive tuning on, the busy-disk throttle off until it has been measured. The detected drives are listed in the Logger at start; per-drive overrides ("treat E: as a hard disk") stay out of the lean UI unless the owner wants them (Q6), and would go in the Advanced table first.

## Not in this proposal

- The write cache D3 describes (still not implemented) and 2.2.1's "Disable Windows caching of disk writes/reads" boxes (unbuffered I/O); they are Disk Cache questions.
- Changing what a check reads: it already reads each piece once.
- CPU load and full-screen detection (owner, Q3: disk I/O only).
- Keeping game ping low by limiting the upload: a network question, not a disk one; proposed separately as [LOWPING.md](LOWPING.md) (D49).

## Order and size

1. Part 1: small to medium, one or two patch versions; the priorities help at once on Windows and macOS, the delay-based yielding everywhere.
2. Part 2: medium (the check queue and its threads, drive keys).
3. Part 3: medium, mostly per-platform detection code and tests on the real machines (this PC, the test lab's Samba share, the Xubuntu VM, a USB disk).
4. Part 4: the largest; needs measurements before choosing its numbers.

Each part bumps the patch version (D37) when it lands. None touches the peer wire, so no interop rerun; parts 2-4 need checks on real disks, which go to BACKLOG "Disk and storage".

## Questions for the owner

- **Q1.** Approve the four parts, and the order 1, 2, 3, 4?
- **Q2.** Defaults: gentle checks and drive tuning on, the busy-disk throttle off?
- **Q3.** Answered: disk I/O only; CPU load is fine.
- **Q4.** Busy signal: our own timings only (every platform, includes NAS), or also the OS counters on Windows and Linux to react only to other programs?
- **Q5.** May the busy-disk throttle slow uploads (it would defer the *disk reads* for seeding while the disk is busy, which lowers the ratio on private trackers), or only downloads? This is about the disk, not about ping; ping is [LOWPING.md](LOWPING.md).
- **Q6.** Per-drive overrides in the UI, in the Advanced table only, or not at all?
- **Q7.** The page: a new "Disk I/O" under Advanced (proposed), or the checking options on Disk Cache?
- **Q8.** Answered: "smart recheck" means gentle checks (lower I/O priority, slower, every tool a platform has), not fewer; part 1 is written that way.

Answers of 2026-11-03_084209_1793691729: Q1 all four parts, in that order, before the next release; Q2 as proposed (gentle checks and drive tuning on, the throttle off); Q4 our own timings only; Q5 both, as two separate boxes; Q6 the Advanced table only (`drive_overrides`); Q7 a new Advanced > Disk I/O page.

## Implementation status

Settings in 1.1.36 (remote protocol 13): `check_gently`, `check_rate_limit` / `check_rate_mb`, `tune_for_drive`, `drive_overrides`, `disk_slow_downloads`, `disk_slow_uploads`; `active_checking` now defaults to 4. The engine in 1.1.37 (`disk/smart.rs`, `engine/disk_limit.rs`):

- **Checks:** the `lt-hash` thread is now a router that finds each check's drive and hands it to one of eight `lt-check-N` threads, the same thread for the same drive, new drives taking the first `active_checking` threads in turn; fences and barriers go to every checking thread in order. One check per drive, also on an NVMe drive (the proposal allowed two there).
- **Gentle checks:** background I/O priority for the check, the check's own file handles with read-ahead and forget hints, and a wait as long as the read after a piece read more than twice the check's usual time per byte (reads faster than 4 GB/s count as the system's cache and are left out). The MB/s cap paces the check from its start. Waits stop at once when the torrent is stopped.
- **Drive tuning:** a hard disk or removable drive takes two disk jobs at once, a share's server four; the jobs wait on the I/O threads. The detected drives go to the Logger ("Disk I/O: drive D: (HDD, SATA)"). Not built: whole-piece read cache lines for hard disks (the read cache's line size stays one setting).
- **Page and detection:** the Disk I/O page in all three GUIs and the Web UI in 1.1.38; drive detection and the platforms' priority calls in 1.1.39 (Windows: volume disk extents and seek penalty, without elevation; UNC and mapped drives are Network without asking the server. Linux: `statfs`, sysfs `rotational`, partitions to their disk, `dm`/`md` slaves, USB as removable. macOS: IOKit's medium type. FreeBSD: `kern.geom.confxml`'s rotation rate, read from the disk's provider since 1.1.40; umass disks are not seen as removable).
- **Busy disk:** while either throttle box is on (nothing is timed otherwise), writes (whole write jobs) and file reads for peers are timed per drive; a drive is busy while the 90th percentile of its last two seconds is more than its allowance above the lowest per-second 10th percentile of three minutes. The engine then halves the download (writes) or upload (reads) rate, holds three seconds, halves again while busy, grows 5% a second when quiet and drops the cap after 30 quiet seconds, never below 50 kB/s. It is one global cap per direction, merged with keep ping low's (the lower wins), shown as the status bar's "(auto ...)" and in the Logger, not a per-drive factor or a "slowed for the disk" tooltip.
- **Open:** measured numbers for the allowances and the throttle on real disks (a hard disk, the Samba share, a USB disk), and drive detection on macOS and FreeBSD hardware (BACKLOG "Disk and storage").

## Sources

- [SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN](https://learn.microsoft.com/windows/win32/api/processthreadsapi/nf-processthreadsapi-setthreadpriority)
- [DEVICE_SEEK_PENALTY_DESCRIPTOR](https://learn.microsoft.com/windows/win32/api/winioctl/ns-winioctl-device_seek_penalty_descriptor)
- [ioprio_set(2)](https://man7.org/linux/man-pages/man2/ioprio_set.2.html)
- [qBittorrent issue 17632: limit disk I/O while rechecking](https://github.com/qbittorrent/qBittorrent/issues/17632)
- [qBittorrent issue 11541: I/O errors on slow hard disks](https://github.com/qbittorrent/qBittorrent/issues/11541)
