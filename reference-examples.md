# Reference examples

Worked examples from the design discussion, kept verbatim for reference while implementing `lyrebird` in Rust. These are illustrative (some were prototyped in bash/Python) — not final Rust code.

## Contact sheet generation (ffmpeg)

Generates a 4x4 grid of thumbnails sampled across a video file, so it can be identified at a glance instead of watched.

```bash
for f in *.mkv; do
  ffmpeg -i "$f" -vf "select='not(mod(n\,1000))',scale=320:-1,tile=4x4" \
    -frames:v 1 -update 1 "${f%.mkv}_sheet.png"
done
```

Notes:
- `mod(n\,1000)` is the frame-sampling interval — pick 16 frames spread across the file. Scale this based on duration (e.g. `mod(n\,3000)` for long files) so tiles are evenly spread.
- `-update 1` avoids an ffmpeg warning about the output not being an image-sequence pattern.
- Output is one PNG per input file — a literal image grid (like an old photography contact sheet), not structured data.

## Quick preview alternative (mpv seek points)

Faster than a full watch-through when you just need to sanity-check content at a couple of timestamps:

```bash
mpv --start=00:05:00 --length=15 file.mkv
```

## Sample TMDB-based manifest

```
title_01.mkv	tv	84958	1	1
title_02.mkv	tv	84958	1	2
title_03.mkv	tv	84958	1	3
special_01.mkv	manual	Show.S00E01.Behind.The.Scenes.mkv	600
movie_rip.mkv	movie	603
```

## Sample resolved RenamePlan (`renames.txt`)

```
title_01.mkv	Show - S01E01 - Pilot.mkv	1320
title_02.mkv	Show - S01E02 - The Reveal.mkv	1350
title_03.mkv	Show - S01E03 - Consequences.mkv	1290
```

## Prototype: Python resolver (build_renames.py)

Early prototype of the "resolve TMDB manifest -> RenamePlan" step, written in Python before the decision to use Rust. Kept here as a reference for the logic/API calls, not as code to port directly.

```python
#!/usr/bin/env python3
import csv
import os
import re
import sys
import requests

TMDB_API_KEY = os.environ["TMDB_API_KEY"]
BASE = "https://api.themoviedb.org/3"

def sanitize(name: str) -> str:
    return re.sub(r'[<>:"/\\|?*]', "", name).strip()

def get_series_name(series_id):
    r = requests.get(f"{BASE}/tv/{series_id}", params={"api_key": TMDB_API_KEY})
    r.raise_for_status()
    return r.json()["name"]

def get_episode(series_id, season, episode):
    r = requests.get(
        f"{BASE}/tv/{series_id}/season/{season}/episode/{episode}",
        params={"api_key": TMDB_API_KEY},
    )
    r.raise_for_status()
    data = r.json()
    return data["name"], data.get("runtime")

def get_movie(movie_id):
    r = requests.get(f"{BASE}/movie/{movie_id}", params={"api_key": TMDB_API_KEY})
    r.raise_for_status()
    data = r.json()
    return data["title"], data.get("runtime"), data.get("release_date", "")[:4]

def main(manifest_path, out_path):
    rows = []
    with open(manifest_path, newline="") as f:
        reader = csv.reader(f, delimiter="\t")
        for line_num, row in enumerate(reader, 1):
            if not row or row[0].startswith("#"):
                continue
            src, kind, *rest = row

            if kind == "tv":
                series_id, season, episode = rest
                series_name = get_series_name(series_id)
                ep_title, runtime = get_episode(series_id, season, episode)
                new_name = f"{sanitize(series_name)} - S{int(season):02d}E{int(episode):02d} - {sanitize(ep_title)}.mkv"
                duration = runtime * 60 if runtime else ""

            elif kind == "movie":
                movie_id = rest[0]
                title, runtime, year = get_movie(movie_id)
                new_name = f"{sanitize(title)} ({year}).mkv"
                duration = runtime * 60 if runtime else ""

            elif kind == "manual":
                new_name = rest[0]
                duration = rest[1] if len(rest) > 1 else ""

            else:
                print(f"line {line_num}: unknown type '{kind}'", file=sys.stderr)
                continue

            rows.append((src, new_name, duration))

    with open(out_path, "w", newline="") as f:
        writer = csv.writer(f, delimiter="\t")
        writer.writerows(rows)

if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
```

## Prototype: bash validate/apply scripts

Early prototype of validation logic (pre-Rust decision). Useful as a checklist of what the Rust `validate` module needs to replicate.

`validate_renames.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

manifest="$1"
tolerance_pct=10
errors=0

declare -A seen_old
declare -A seen_new

line_num=0
while IFS=$'\t' read -r old new expected_dur; do
  line_num=$((line_num+1))
  [[ -z "$old" || -z "$new" ]] && continue

  if [[ -n "${seen_old[$old]:-}" ]]; then
    echo "ERROR line $line_num: '$old' already appears on line ${seen_old[$old]}"
    errors=$((errors+1))
  fi
  seen_old[$old]=$line_num

  if [[ -n "${seen_new[$new]:-}" ]]; then
    echo "ERROR line $line_num: target '$new' collides with line ${seen_new[$new]}"
    errors=$((errors+1))
  fi
  seen_new[$new]=$line_num

  if [[ "$old" == "$new" ]]; then
    echo "ERROR line $line_num: source and target are identical ('$old')"
    errors=$((errors+1))
  fi

  if [[ ! -f "$old" ]]; then
    echo "ERROR line $line_num: source file '$old' does not exist"
    errors=$((errors+1))
  fi

  if [[ -e "$new" ]]; then
    echo "ERROR line $line_num: target '$new' already exists"
    errors=$((errors+1))
  fi

  if [[ "$new" == */* ]]; then
    echo "ERROR line $line_num: target '$new' contains a path separator"
    errors=$((errors+1))
  fi
  if [[ "$new" != *.mkv && "$new" != *.mp4 ]]; then
    echo "WARNING line $line_num: target '$new' has an unexpected extension"
  fi

  if [[ -n "${expected_dur:-}" && -f "$old" ]]; then
    actual_dur=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$old" 2>/dev/null)
    actual_dur_int=${actual_dur%.*}

    if [[ -n "$actual_dur_int" ]]; then
      diff=$(( actual_dur_int > expected_dur ? actual_dur_int - expected_dur : expected_dur - actual_dur_int ))
      allowed=$(( expected_dur * tolerance_pct / 100 ))
      [[ $allowed -lt 30 ]] && allowed=30

      if [[ $diff -gt $allowed ]]; then
        echo "ERROR line $line_num: '$old' duration ${actual_dur_int}s differs from expected ${expected_dur}s by ${diff}s (allowed ${allowed}s) — possible mismatch with '$new'"
        errors=$((errors+1))
      fi
    else
      echo "WARNING line $line_num: could not read duration for '$old'"
    fi
  fi

done < "$manifest"

if [[ $errors -gt 0 ]]; then
  echo "---"
  echo "$errors error(s) found. Fix the manifest before renaming."
  exit 1
else
  echo "Manifest OK: all checks passed."
fi
```

`apply_renames.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail
manifest="$1"

while IFS=$'\t' read -r old new; do
  [[ -z "$old" || -z "$new" ]] && continue
  mv -n -- "$old" "$new"
  echo "renamed: $old -> $new"
done < "$manifest"
```
