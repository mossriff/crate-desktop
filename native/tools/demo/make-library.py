#!/usr/bin/env python3
"""Make a fake music library for screenshots: real album details and covers from Deezer, but every
song is a tiny silent MP3 of the right length. Nothing real is downloaded.

usage: make-library.py OUT_DIR      (already-made albums are skipped)
"""
import json, os, subprocess, sys, time, urllib.parse, urllib.request

ALBUMS = [
    ("Radiohead", "OK Computer"),
    ("Radiohead", "In Rainbows"),
    ("Radiohead", "Kid A"),
    ("Portishead", "Dummy"),
    ("Massive Attack", "Mezzanine"),
    ("Björk", "Homogenic"),
    ("Daft Punk", "Discovery"),
    ("Bon Iver", "For Emma, Forever Ago"),
    ("Cocteau Twins", "Heaven or Las Vegas"),
    ("Boards of Canada", "Music Has the Right to Children"),
    ("Mitski", "Be the Cowboy"),
    ("Talking Heads", "Remain in Light"),
]


def get(path):
    with urllib.request.urlopen("https://api.deezer.com" + path, timeout=20) as r:
        d = json.load(r)
    if "error" in d:
        raise RuntimeError(f"{path}: {d['error']}")
    time.sleep(0.15)
    return d


def safe(s):
    return "".join("_" if c in '/\\:*?"<>|' else c for c in s).strip()


def find(artist, title):
    """The plain edition: an exact title match, else the shortest title starting with it."""
    q = urllib.parse.quote(f"{artist} {title}")
    hits = [a for a in get(f"/search/album?q={q}&limit=25")["data"]
            if a["artist"]["name"].lower() == artist.lower() and a["title"].lower().startswith(title.lower())]
    if not hits:
        raise RuntimeError(f"not on Deezer: {artist} - {title}")
    return min(hits, key=lambda a: (a["title"].lower() != title.lower(), len(a["title"])))["id"]


def main(out):
    os.makedirs(out, exist_ok=True)
    for artist, title in ALBUMS:
        folder = os.path.join(out, safe(artist), safe(title))
        if os.path.exists(os.path.join(folder, ".done")):
            continue
        os.makedirs(folder, exist_ok=True)
        alb = get(f"/album/{find(artist, title)}")
        with urllib.request.urlopen(alb["cover_xl"], timeout=20) as r, open(os.path.join(folder, "cover.jpg"), "wb") as f:
            f.write(r.read())
        tracks = get(f"/album/{alb['id']}/tracks?limit=100")["data"]
        year = alb.get("release_date", "")[:4]
        genre = (alb.get("genres", {}).get("data") or [{}])[0].get("name", "")
        for t in tracks:
            n, disc = t["track_position"], t["disk_number"]
            path = os.path.join(folder, f"{n:02d} {safe(t['title'])}.mp3")
            tags = {"title": t["title"], "artist": t["artist"]["name"], "album_artist": artist, "album": alb["title"],
                    "track": f"{n}/{len(tracks)}", "disc": str(disc), "date": year, "genre": genre}
            cmd = ["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "anullsrc=r=8000:cl=mono", "-t", str(t["duration"]),
                   "-b:a", "8k", "-id3v2_version", "3"]
            for k, v in tags.items():
                cmd += ["-metadata", f"{k}={v}"]
            subprocess.run(cmd + [path], check=True)
        open(os.path.join(folder, ".done"), "w").close()
        print(f"{artist} - {alb['title']}: {len(tracks)} songs")


if __name__ == "__main__":
    main(sys.argv[1])
