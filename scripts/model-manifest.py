"""Hash a converted model and prepare a pinned manifest without uploading it."""
import argparse
import hashlib
import json
import pathlib
import re
from urllib.parse import urlsplit

FILES = ("config.json", "model.bin", "shared_vocabulary.json", "sentencepiece.bpe.model")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("model", type=pathlib.Path)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--id", default="m2m100-418m-int8-v1")
    parser.add_argument("--expect", type=pathlib.Path, help="Verify identity, sizes and hashes against a reviewed manifest")
    parser.add_argument("--output", type=pathlib.Path, help="Atomically save JSON instead of printing it")
    args = parser.parse_args()
    url = urlsplit(args.base_url)
    if url.scheme != "https" or not url.hostname or url.username or url.password or url.query or url.fragment:
        parser.error("base URL must be HTTPS without credentials, query or fragment")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,99}", args.id):
        parser.error("model ID must be a safe path segment")
    files = []
    for name in FILES:
        path = args.model / name
        if not path.is_file() or path.is_symlink():
            parser.error(f"missing ordinary model file: {name}")
        with path.open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        files.append({"name": name, "url": args.base_url.rstrip("/") + "/" + name,
                      "sha256": digest, "bytes": path.stat().st_size})
    manifest = {"id": args.id, "bytes": sum(file["bytes"] for file in files), "files": files}
    if args.expect:
        expected = json.loads(args.expect.read_text())
        identity = lambda value: (value["id"], value["bytes"], sorted(
            (file["name"], file["bytes"], file["sha256"]) for file in value["files"]))
        if identity(manifest) != identity(expected):
            parser.error("model files differ from the reviewed manifest; no output was written")
    output = json.dumps(manifest, indent=2) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        temporary = args.output.with_suffix(args.output.suffix + ".tmp")
        temporary.write_text(output)
        temporary.replace(args.output)
    else:
        print(output, end="")


if __name__ == "__main__":
    main()
