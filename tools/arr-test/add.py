#!/usr/bin/env python3
"""Adds the test series and movie (if missing) and searches for them."""

import sys
import urllib.parse

sys.argv = ["arr.py"]
import arr  # noqa: E402

_, series_list = arr.call("sonarr", "GET", "series")
series = next((s for s in series_list if s["title"] == "Fawlty Towers"), None)
if series is None:
    _, res = arr.call("sonarr", "GET", "series/lookup?term=" + urllib.parse.quote("Fawlty Towers"))
    s = res[0]
    s.update(qualityProfileId=1, rootFolderPath="/srv/media/tv", monitored=True, seasonFolder=True,
             addOptions={"monitor": "all", "searchForMissingEpisodes": False})
    st, series = arr.call("sonarr", "POST", "series", s)
    print("add series", st)
_, eps = arr.call("sonarr", "GET", f"episode?seriesId={series['id']}")
e = next(x for x in eps if x["seasonNumber"] == 1 and x["episodeNumber"] == 1)
print("episode", e["id"], e["title"])
print("episode search", arr.call("sonarr", "POST", "command", {"name": "EpisodeSearch", "episodeIds": [e["id"]]})[0])

_, movies = arr.call("radarr", "GET", "movie")
movie = next((m for m in movies if m["title"] == "Night of the Living Dead" and m["year"] == 1968), None)
if movie is None:
    _, res = arr.call("radarr", "GET", "movie/lookup?term=" + urllib.parse.quote("Night of the Living Dead 1968"))
    m = next(x for x in res if x["year"] == 1968)
    m.update(qualityProfileId=1, rootFolderPath="/srv/media/movies", monitored=True,
             minimumAvailability="released", addOptions={"searchForMovie": False})
    st, movie = arr.call("radarr", "POST", "movie", m)
    print("add movie", st)
print("movie search", arr.call("radarr", "POST", "command", {"name": "MoviesSearch", "movieIds": [movie["id"]]})[0])
