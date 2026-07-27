#!/usr/bin/env python3
"""Retroactively correct file extensions for trackerdl downloads.

trackerdl embeds the host's original filename in angle brackets, e.g.:
    "33 [V1] <33 ideas 20141105 202030.m4a>.mp3"
Older trackerdl builds always appended .mp3 regardless of the real format.
This walks a directory tree and renames the trailing extension to match
the one inside the angle brackets, when they differ.
"""
import argparse
import os
import re
import sys

BRACKET_RE = re.compile(r"<([^<>]+)>\.[^./]+$")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "root", nargs="?", default="/Volumes/jaredmega/MEGAMUSIC/trackerdl"
    )
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    if not os.path.isdir(args.root):
        print(f"error: {args.root} is not a directory", file=sys.stderr)
        sys.exit(1)

    checked = fixed = skipped_no_bracket = skipped_no_ext = 0

    for dirpath, _dirnames, filenames in os.walk(args.root):
        for name in filenames:
            checked += 1
            m = BRACKET_RE.search(name)
            if not m:
                skipped_no_bracket += 1
                continue
            original = m.group(1)
            orig_ext = os.path.splitext(original)[1].lstrip(".").lower()
            if not orig_ext:
                skipped_no_ext += 1
                continue
            current_ext = os.path.splitext(name)[1].lstrip(".").lower()
            if current_ext == orig_ext:
                continue
            base = name[: -(len(current_ext) + 1)] if current_ext else name
            new_name = f"{base}.{orig_ext}"
            old_path = os.path.join(dirpath, name)
            new_path = os.path.join(dirpath, new_name)
            if os.path.exists(new_path):
                print(f"skip (target exists): {old_path}", file=sys.stderr)
                continue
            fixed += 1
            print(f"{'[dry-run] ' if args.dry_run else ''}{name}  ->  {new_name}")
            if not args.dry_run:
                os.rename(old_path, new_path)

    print(
        f"\nchecked {checked} files: {fixed} renamed, "
        f"{skipped_no_bracket} had no bracketed original filename, "
        f"{skipped_no_ext} bracketed name had no extension."
    )


if __name__ == "__main__":
    main()
