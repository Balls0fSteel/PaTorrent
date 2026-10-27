#!/usr/bin/env python3
"""summarize.py: medians of bench.py's JSON lines, as Markdown (docs) or HTML (site).

    summarize.py results.jsonl [--html | --bars]
    summarize.py results.jsonl --fill site/benchmarks.html   # the charts and tables in place
"""

import json
import statistics
import sys

ORDER = ["patorrent", "qbittorrent", "transmission", "deluge", "rtorrent"]
NAMES = {"patorrent": "PaTorrent", "qbittorrent": "qBittorrent", "transmission": "Transmission",
         "deluge": "Deluge", "rtorrent": "rTorrent"}


def load(path):
    with open(path) as f:
        return [json.loads(line) for line in f if line.strip()]


def med(rows, key):
    vals = [r[key] for r in rows if isinstance(r.get(key), (int, float))]
    return statistics.median(vals) if vals else None


def spread(rows, key):
    vals = [r[key] for r in rows if isinstance(r.get(key), (int, float))]
    return (min(vals), max(vals)) if vals else (None, None)


def fmt(v, digits=1):
    if v is None:
        return "–"
    if v >= 100:
        return f"{v:,.0f}"
    return f"{v:,.{digits}f}"


def transfer_table(rows):
    out = [["Client", "Version", "Runs", "Throughput (MiB/s)", "range", "CPU s per GiB", "range",
            "First byte to done (s)"]]
    for c in ORDER:
        rs = [r for r in rows if r["scenario"] == "transfer" and r["client"] == c]
        if not rs:
            continue
        ok = [r for r in rs if "error" not in r and r.get("verified")]
        lo, hi = spread(ok, "mib_per_s")
        clo, chi = spread(ok, "cpu_s_per_gib")
        version = next((r["version"] for r in rs if "version" in r), "")
        for r in ok:
            r["done_s"] = r["seconds"] - r["first_byte_s"]
        out.append([NAMES[c], version, f"{len(ok)} of {len(rs)}", fmt(med(ok, "mib_per_s")),
                    f"{fmt(lo)}–{fmt(hi)}", fmt(med(ok, "cpu_s_per_gib"), 2), f"{fmt(clo, 2)}–{fmt(chi, 2)}",
                    fmt(med(ok, "done_s"))])
    return out


def many_table(rows, count):
    out = [["Client", "Add (s)", "Start with all listed (s)", "Memory, RSS (MB)", "PSS (MB)",
            "Idle CPU (%)", "Empty RSS (MB)"]]
    for c in ORDER:
        rs = [r for r in rows if r["scenario"] == "many" and r["client"] == c and r["count"] == count]
        if not rs:
            continue
        r = rs[-1]
        if "error" in r:
            out.append([NAMES[c], r["error"][:60], "", "", "", "", ""])
            continue
        out.append([NAMES[c], fmt(r["add_s"]), fmt(r["restart_s"], 2), fmt(r["restarted_rss_mb"]),
                    fmt(r["restarted_pss_mb"]), fmt(r["restarted_idle_cpu_pct"], 2), fmt(r["empty_rss_mb"])])
    return out


def markdown(table):
    lines = ["| " + " | ".join(table[0]) + " |", "|" + "---|" * len(table[0])]
    lines += ["| " + " | ".join(row) + " |" for row in table[1:]]
    return "\n".join(lines)


def html(table):
    import html as h
    head = "".join(f'<th scope="col">{h.escape(c)}</th>' for c in table[0])
    body = []
    for row in table[1:]:
        cells = [f'<td class="client">{h.escape(row[0])}</td>']
        cells += [f'<td class="num">{h.escape(v)}</td>' if v[:1].isdigit() or v[:1] == "–"
                  else f"<td>{h.escape(v)}</td>" for v in row[1:]]
        body.append("<tr>" + "".join(cells) + "</tr>")
    return (f'<table class="listview bench">\n<thead><tr>{head}</tr></thead>\n<tbody>\n'
            + "\n".join(body) + "\n</tbody>\n</table>")


def bars(caption, note, values, unit, digits=0):
    """A bar chart in plain HTML: one bar per client, PaTorrent's in the accent colour."""
    import html as h
    top = max(v for _, v in values if v is not None)
    items = []
    for c, v in values:
        width = max(100 * v / top, 0.6)
        cls = ' class="pa"' if c == "patorrent" else ""
        text = f"{v:,.{digits}f} {unit}".strip()
        items.append(f'<li{cls}><span class="who">{NAMES[c]}</span><span class="track">'
                     f'<span class="bar" style="width:{width:.1f}%"></span></span>'
                     f'<span class="val">{h.escape(text)}</span></li>')
    return (f'<figure class="chart">\n<figcaption><strong>{h.escape(caption)}</strong> '
            f'<span>{h.escape(note)}</span></figcaption>\n<ul class="bars">\n'
            + "\n".join(items) + "\n</ul>\n</figure>")


def fill(page, rows):
    """Replaces what stands between `<!-- bench:NAME -->` and `<!-- /bench:NAME -->` in `page`."""
    import re
    blocks = {f"chart-{i}": c for i, c in enumerate(charts(rows).split("\n\n"))}
    blocks["table-transfer"] = html(transfer_table(rows))
    for count in {r["count"] for r in rows if r["scenario"] == "many"}:
        blocks[f"table-{count}"] = html(many_table(rows, count))
    with open(page, encoding="utf-8") as f:
        text = f.read()
    for name, block in blocks.items():
        text, n = re.subn(rf"(<!-- bench:{name} -->).*?(<!-- /bench:{name} -->)",
                          lambda m: f"{m[1]}\n{block}\n{m[2]}", text, flags=re.S)
        if n != 1:
            print(f"{page}: no bench:{name} block", file=sys.stderr)
    with open(page, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)


def charts(rows):
    t = [r for r in rows if r["scenario"] == "transfer" and "error" not in r and r.get("verified")]
    m = {r["client"]: r for r in rows if r["scenario"] == "many" and r["count"] == 10000 and "error" not in r}

    def tm(key):
        return [(c, med([r for r in t if r["client"] == c], key)) for c in ORDER]

    def mm(key):
        return [(c, m[c][key]) for c in ORDER if c in m]
    return "\n\n".join([
        bars("Memory with 10,000 torrents, after a restart", "RSS, lower is better", mm("restarted_rss_mb"), "MB"),
        bars("Memory right after adding 10,000 torrents", "RSS, lower is better", mm("added_rss_mb"), "MB"),
        bars("Start until all 10,000 are listed", "seconds, lower is better", mm("restart_s"), "s", 2),
        bars("Download speed from one peer", "MiB/s, median of 5, higher is better", tm("mib_per_s"), "MiB/s"),
        bars("CPU time per GiB downloaded", "seconds, median of 5, lower is better", tm("cpu_s_per_gib"), "s", 2),
    ])


def main():
    rows = load(sys.argv[1])
    if "--fill" in sys.argv:
        fill(sys.argv[sys.argv.index("--fill") + 1], rows)
        return
    if "--bars" in sys.argv:
        print(charts(rows))
        return
    render = html if "--html" in sys.argv else markdown
    print("## Transfer\n")
    print(render(transfer_table(rows)))
    for count in sorted({r["count"] for r in rows if r["scenario"] == "many"}):
        print(f"\n## {count:,} torrents\n")
        print(render(many_table(rows, count)))


if __name__ == "__main__":
    main()
