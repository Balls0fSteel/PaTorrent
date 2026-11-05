# closing-test

A hand check of D47's closing window in the GTK GUI with real, slow storage moves, on a Linux
machine with an X server, `Xvfb`, `xdotool`, ImageMagick's `import`, `python3` and passwordless
`sudo` (the owner's Xubuntu test VM). Nothing is installed; `cleanup` undoes everything.

What it sets up:

- `N` (default 5) files of `SIZE_MB` (default 120) random bytes in a throwaway download folder,
  with a `.torrent` each (`mktorrent.py`), and settings that auto-load them and move completed
  downloads to `/mnt/ptest/done`, a loop-mounted 2 GB ext4 image (another filesystem, so the
  move copies).
- The app runs on display `:78` in a transient systemd scope with reads from `/dev/sda`
  throttled to 25 MB/s and memory capped at 400 MB, so the checks and copies cannot come from
  the page cache and take a while. Below about 400 MB a debug build is OOM-killed in the scope;
  with the cap, the first frame of a new window can take seconds (GTK loads its files at the
  throttled rate).

```sh
N=4 SIZE_MB=400 ./closing-test.sh setup
./closing-test.sh start          # returns once the first move has started
./closing-test.sh term           # SIGTERM: the app quits as File > Exit does
./closing-test.sh shot moving    # run/moving.png
DISPLAY=:78 xdotool mousemove 559 330 click 1   # Quit Now (the window has no title bar here)
./closing-test.sh cleanup
```

`APP` names the `rookparcel-gtk` binary (default: a debug build in `~/rookparcel-sec/target`).
Afterwards check the destination (no partial copies after Quit Now), the sources, and that a
restart finds every torrent seeding.
