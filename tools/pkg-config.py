from __future__ import annotations

import os
import re
import sys
from dataclasses import dataclass
from pathlib import Path


VERSION_PATTERN = re.compile(r"^\s*([^\s<>=!,]+)\s*(>=|<=|=|<|>)?\s*(.*?)\s*$")
VARIABLE_PATTERN = re.compile(r"\$\{([^}]+)\}")
WINDOWS_VERSIONED_LIB_PATTERN = re.compile(r"^(?P<base>.+)-\d+\.\d+$")
GTK_PATH_HINT_PATTERN = re.compile(
    r"gtk|glib|gobject|gio|gdk|gsk|pango|graphene|cairo|vcpkg", re.IGNORECASE
)


@dataclass
class PackageSpec:
    name: str
    op: str | None = None
    version: str | None = None


@dataclass
class PcPackage:
    name: str
    version: str
    libs: list[str]
    libs_private: list[str]
    cflags: list[str]
    requires: list[PackageSpec]
    requires_private: list[PackageSpec]


def default_triplet() -> str:
    return os.environ.get("VCPKG_DEFAULT_TRIPLET", "x64-windows")


def workspace_root() -> Path:
    return Path(__file__).resolve().parent.parent


def repo_install_root() -> Path:
    return workspace_root() / "vcpkg_installed"


def candidate_triplets() -> list[str]:
    triplets = [default_triplet()]
    path_patterns = re.compile(
        r"installed[\\/](?P<triplet>[^\\/]+)[\\/](?:debug[\\/])?lib"
    )
    for variable in ("PKG_CONFIG_PATH", "PKG_CONFIG_LIBDIR", "PATH"):
        raw = os.environ.get(variable, "")
        for part in parse_path_list(raw):
            match = path_patterns.search(str(part))
            if match is not None:
                triplets.append(match.group("triplet"))
    installed_root = vcpkg_root() / "installed"
    if installed_root.is_dir():
        triplets.extend(
            entry.name for entry in sorted(installed_root.iterdir()) if entry.is_dir()
        )
    repo_root = repo_install_root()
    if repo_root.is_dir():
        triplets.extend(
            entry.name for entry in sorted(repo_root.iterdir()) if entry.is_dir()
        )
    return dedupe(triplets)


def release_prefix(prefix: Path) -> Path:
    if prefix.name.lower() == "debug":
        return prefix.parent / "release"
    return prefix


def debug_prefix(prefix: Path) -> Path:
    if prefix.name.lower() == "release":
        return prefix.parent / "debug"
    if prefix.name.lower() == "debug":
        return prefix
    return prefix / "debug"


def candidate_prefixes() -> list[Path]:
    prefixes: list[Path] = []

    install_root = repo_install_root()
    if install_root.is_dir():
        for triplet in candidate_triplets():
            prefix = install_root / triplet
            if prefix.is_dir():
                prefixes.append(prefix)

    root = os.environ.get("VCPKG_ROOT")
    if root:
        root_path = Path(root)
        installed_root = root_path / "installed"
        if installed_root.is_dir():
            for triplet in candidate_triplets():
                prefixes.append(installed_root / triplet)

    for part in parse_path_list(os.environ.get("PATH", "")):
        text = str(part)
        lowered = text.lower()
        if part.name.lower() == "bin" and GTK_PATH_HINT_PATTERN.search(text):
            prefixes.append(part.parent)
            continue
        if "vcpkg" in lowered and (part / "installed").is_dir():
            for triplet in candidate_triplets():
                prefixes.append(part / "installed" / triplet)

    return dedupe_paths(prefixes)


def active_prefix() -> Path:
    for prefix in candidate_prefixes():
        release = release_prefix(prefix)
        if (release / "lib").is_dir() or (release / "include").is_dir():
            return prefix
    return active_vcpkg_root() / "installed" / default_triplet()


def vcpkg_root() -> Path:
    root = os.environ.get("VCPKG_ROOT")
    if not root:
        raise FileNotFoundError("VCPKG_ROOT is not set")
    return Path(root)


def active_vcpkg_root() -> Path:
    root = vcpkg_root()
    if (root / "installed").is_dir():
        return root

    patterns = re.compile(
        r"^(?P<root>.*?[\\/])installed[\\/][^\\/]+[\\/](?:debug[\\/])?lib(?:[\\/].*)?$"
    )
    for variable in ("PKG_CONFIG_PATH", "PKG_CONFIG_LIBDIR", "PATH"):
        raw = os.environ.get(variable, "")
        for part in parse_path_list(raw):
            match = patterns.match(str(part))
            if match is None:
                continue
            candidate = normalize_path(match.group("root").rstrip("\\/"))
            if (candidate / "installed").is_dir():
                return candidate

    return root


def vcpkg_lib_dir(triplet: str | None = None) -> Path:
    return active_vcpkg_root() / "installed" / (triplet or default_triplet()) / "lib"


def vcpkg_debug_lib_dir(triplet: str | None = None) -> Path:
    return (
        active_vcpkg_root()
        / "installed"
        / (triplet or default_triplet())
        / "debug"
        / "lib"
    )


def vcpkg_include_dir(triplet: str | None = None) -> Path:
    return (
        active_vcpkg_root() / "installed" / (triplet or default_triplet()) / "include"
    )


def link_dirs_for_prefix(prefix: Path) -> list[Path]:
    release_lib = release_prefix(prefix) / "lib"
    debug_lib = debug_prefix(prefix) / "lib"
    release_manual = release_lib / "manual-link"
    debug_manual = debug_lib / "manual-link"
    debug_build = is_debug_build()
    if debug_build:
        return existing_paths([debug_lib, debug_manual, release_lib, release_manual])
    return existing_paths([release_lib, release_manual, debug_lib, debug_manual])


def link_dirs(triplet: str | None = None) -> list[Path]:
    return link_dirs_for_prefix(
        active_vcpkg_root() / "installed" / (triplet or default_triplet())
    )


def all_link_dirs() -> list[Path]:
    directories: list[Path] = []
    for prefix in candidate_prefixes():
        directories.extend(link_dirs_for_prefix(prefix))
    for triplet in candidate_triplets():
        directories.extend(link_dirs(triplet))
    return dedupe_paths(directories)


def pkgconfig_dirs_for_prefix(prefix: Path) -> list[Path]:
    release = release_prefix(prefix)
    debug = debug_prefix(prefix)
    if is_debug_build():
        return existing_paths(
            [
                debug / "lib" / "pkgconfig",
                release / "lib" / "pkgconfig",
                release / "share" / "pkgconfig",
                debug / "share" / "pkgconfig",
            ]
        )
    return existing_paths(
        [
            release / "lib" / "pkgconfig",
            release / "share" / "pkgconfig",
            debug / "lib" / "pkgconfig",
            debug / "share" / "pkgconfig",
        ]
    )


def is_debug_build() -> bool:
    return os.environ.get("DEBUG", "").lower() in {"1", "true", "yes", "on"}


def main() -> int:
    args = sys.argv[1:]
    if not args:
        return 0

    if "--version" in args:
        print("shellx-pkg-config 0.1")
        return 0

    flags = {arg for arg in args if arg.startswith("--")}
    specs = [parse_spec(arg) for arg in args if not arg.startswith("-")]

    if not specs:
        return 0

    try:
        packages = load_requested_packages(specs)
    except Exception as error:  # noqa: BLE001
        print(str(error), file=sys.stderr)
        return 1

    if "--exists" in flags:
        return 0

    if "--modversion" in flags:
        for package in packages:
            print(package.version)
        return 0

    output: list[str] = []
    if "--cflags" in flags:
        output.extend(
            collect_flags(
                packages,
                include_libs=False,
                include_cflags=True,
                is_static="--static" in flags,
            )
        )
    if "--libs" in flags:
        output.extend(
            collect_flags(
                packages,
                include_libs=True,
                include_cflags=False,
                is_static="--static" in flags,
            )
        )

    if output:
        print(" ".join(output))
    return 0


def load_requested_packages(specs: list[PackageSpec]) -> list[PcPackage]:
    seen: dict[str, PcPackage] = {}
    resolved: list[PcPackage] = []
    for spec in specs:
        package = load_package(spec.name, seen)
        ensure_version(package, spec)
        if package.name not in [item.name for item in resolved]:
            resolved.append(package)
    return resolved


def load_package(name: str, seen: dict[str, PcPackage]) -> PcPackage:
    if name in seen:
        return seen[name]

    pc_path = find_pc_file(name)
    if pc_path is not None:
        package = parse_pc_file(pc_path)
        seen[name] = package
        return package

    package = builtin_package(name)
    if package is not None:
        seen[name] = package
        return package

    raise FileNotFoundError(f"Package '{name}' not found in pkg-config search paths")


def builtin_package(name: str) -> PcPackage | None:
    prefix = active_prefix()
    include = path_flag(release_prefix(prefix) / "include")
    lib = path_flag(release_prefix(prefix) / "lib")
    shared_link = [f"-L{path_flag(directory)}" for directory in all_link_dirs()]

    packages: dict[str, PcPackage] = {
        "glib-2.0": PcPackage(
            name="glib-2.0",
            version="2.80.0",
            libs=[*shared_link, link_flag("glib-2.0")],
            libs_private=[],
            cflags=[f"-I{include}/glib-2.0", f"-I{lib}/glib-2.0/include"],
            requires=[],
            requires_private=[],
        ),
        "gmodule-2.0": PcPackage(
            name="gmodule-2.0",
            version="2.80.0",
            libs=[*shared_link, link_flag("gmodule-2.0")],
            libs_private=[],
            cflags=[],
            requires=[PackageSpec("glib-2.0")],
            requires_private=[],
        ),
        "gobject-2.0": PcPackage(
            name="gobject-2.0",
            version="2.80.0",
            libs=[*shared_link, link_flag("gobject-2.0")],
            libs_private=[],
            cflags=[],
            requires=[PackageSpec("glib-2.0")],
            requires_private=[],
        ),
        "gio-2.0": PcPackage(
            name="gio-2.0",
            version="2.80.0",
            libs=[*shared_link, link_flag("gio-2.0")],
            libs_private=[],
            cflags=[],
            requires=[PackageSpec("gobject-2.0"), PackageSpec("glib-2.0")],
            requires_private=[],
        ),
        "cairo": PcPackage(
            name="cairo",
            version="1.18.0",
            libs=[*shared_link, link_flag("cairo")],
            libs_private=[],
            cflags=[f"-I{include}/cairo"],
            requires=[PackageSpec("glib-2.0")],
            requires_private=[],
        ),
        "cairo-gobject": PcPackage(
            name="cairo-gobject",
            version="1.18.0",
            libs=[*shared_link, link_flag("cairo-gobject")],
            libs_private=[],
            cflags=[f"-I{include}/cairo"],
            requires=[PackageSpec("cairo"), PackageSpec("gobject-2.0")],
            requires_private=[],
        ),
        "pango": PcPackage(
            name="pango",
            version="1.52.0",
            libs=[*shared_link, link_flag("pango-1.0")],
            libs_private=[],
            cflags=[f"-I{include}/pango-1.0"],
            requires=[PackageSpec("gobject-2.0"), PackageSpec("glib-2.0")],
            requires_private=[],
        ),
        "pangocairo": PcPackage(
            name="pangocairo",
            version="1.52.0",
            libs=[*shared_link, link_flag("pangocairo-1.0")],
            libs_private=[],
            cflags=[f"-I{include}/pango-1.0"],
            requires=[PackageSpec("pango"), PackageSpec("cairo")],
            requires_private=[],
        ),
        "gdk-pixbuf-2.0": PcPackage(
            name="gdk-pixbuf-2.0",
            version="2.42.10",
            libs=[*shared_link, link_flag("gdk_pixbuf-2.0")],
            libs_private=[],
            cflags=[f"-I{include}/gdk-pixbuf-2.0"],
            requires=[
                PackageSpec("gobject-2.0"),
                PackageSpec("gio-2.0"),
                PackageSpec("glib-2.0"),
            ],
            requires_private=[],
        ),
        "graphene-1.0": PcPackage(
            name="graphene-1.0",
            version="1.10.8",
            libs=[*shared_link, link_flag("graphene-1.0")],
            libs_private=[],
            cflags=[f"-I{include}/graphene-1.0"],
            requires=[],
            requires_private=[],
        ),
        "graphene-gobject-1.0": PcPackage(
            name="graphene-gobject-1.0",
            version="1.10.8",
            libs=[*shared_link, link_flag("graphene-gobject-1.0")],
            libs_private=[],
            cflags=[f"-I{include}/graphene-1.0"],
            requires=[PackageSpec("graphene-1.0"), PackageSpec("gobject-2.0")],
            requires_private=[],
        ),
        "gtk4": PcPackage(
            name="gtk4",
            version="4.14.0",
            libs=[
                *shared_link,
                link_flag("gtk-4"),
                link_flag("gdk-4"),
                link_flag("gsk-4"),
            ],
            libs_private=[],
            cflags=[f"-I{include}/gtk-4.0"],
            requires=[
                PackageSpec("gio-2.0"),
                PackageSpec("gdk-pixbuf-2.0"),
                PackageSpec("pango"),
                PackageSpec("pangocairo"),
                PackageSpec("cairo"),
                PackageSpec("cairo-gobject"),
                PackageSpec("graphene-gobject-1.0"),
            ],
            requires_private=[],
        ),
    }

    return packages.get(name)


def link_flag(name: str) -> str:
    return f"-l{name}"


def windows_link_name(name: str) -> str:
    if os.name != "nt":
        return name
    match = WINDOWS_VERSIONED_LIB_PATTERN.match(name)
    if match is None:
        return name
    return match.group("base")


def path_flag(path: Path) -> str:
    return path.as_posix()


def candidate_library_names(name: str) -> list[str]:
    candidates = [name]
    base = windows_link_name(name)
    if base != name:
        candidates.append(base)
    if not name.startswith("lib"):
        candidates.append(f"lib{name}")
    if base != name and not base.startswith("lib"):
        candidates.append(f"lib{base}")
    return dedupe(candidates)


def library_matches(stem: str, candidate: str) -> bool:
    variants = [stem]
    if stem.startswith("lib"):
        variants.append(stem[3:])
    return any(
        variant == candidate or variant.startswith(f"{candidate}-")
        for variant in dedupe(variants)
    )


def find_matching_library(directory: Path, candidate: str) -> str | None:
    if not directory.is_dir():
        return None
    for path in sorted(directory.glob("*.lib")):
        if library_matches(path.stem, candidate):
            return path.stem
    return None


def resolve_library_name(name: str, search_dirs: list[Path]) -> str:
    if os.name != "nt":
        return name

    checked: list[str] = []
    ordered_dirs = dedupe_paths([*search_dirs, *all_link_dirs()])
    for candidate in candidate_library_names(name):
        for directory in ordered_dirs:
            path = directory / f"{candidate}.lib"
            checked.append(str(path))
            if path.is_file():
                return candidate

    for candidate in candidate_library_names(name):
        for directory in ordered_dirs:
            matched = find_matching_library(directory, candidate)
            if matched is not None:
                return matched

    preview = ", ".join(checked[:12])
    if len(checked) > 12:
        preview += ", ..."
    raise FileNotFoundError(
        f"Windows library '{name}' not found under configured search paths. Tried: {preview}"
    )


def dedupe_paths(paths: list[Path]) -> list[Path]:
    seen: set[str] = set()
    output: list[Path] = []
    for path in paths:
        key = str(path)
        if key in seen:
            continue
        seen.add(key)
        output.append(path)
    return output


def existing_paths(paths: list[Path]) -> list[Path]:
    return [path for path in dedupe_paths(paths) if path.exists()]


def normalize_library_flags(flags: list[str]) -> list[str]:
    if os.name != "nt":
        return flags

    output: list[str] = []
    for flag in flags:
        if flag.startswith("-L"):
            path = normalize_path(flag[2:])
            output.append(f"-L{path_flag(path)}")
            continue
        output.append(flag)
    return output


def collect_flags(
    packages: list[PcPackage],
    *,
    include_libs: bool,
    include_cflags: bool,
    is_static: bool,
) -> list[str]:
    resolved: list[str] = []
    visited: set[str] = set()
    cache: dict[str, PcPackage] = {package.name: package for package in packages}
    for package in packages:
        collect_package_flags(
            package, resolved, visited, cache, include_libs, include_cflags, is_static
        )
    deduped = dedupe(resolved)
    if include_libs:
        return normalize_library_flags(deduped)
    return deduped


def collect_package_flags(
    package: PcPackage,
    output: list[str],
    visited: set[str],
    cache: dict[str, PcPackage],
    include_libs: bool,
    include_cflags: bool,
    is_static: bool,
) -> None:
    if package.name in visited:
        return
    visited.add(package.name)

    dependencies = list(package.requires)
    if is_static:
        dependencies.extend(package.requires_private)

    for spec in dependencies:
        dependency = cache.get(spec.name)
        if dependency is None:
            dependency = load_package(spec.name, cache)
            cache[dependency.name] = dependency
        ensure_version(dependency, spec)
        collect_package_flags(
            dependency, output, visited, cache, include_libs, include_cflags, is_static
        )

    if include_cflags:
        output.extend(package.cflags)
    if include_libs:
        output.extend(package.libs)
        if is_static:
            output.extend(package.libs_private)


def parse_pc_file(path: Path) -> PcPackage:
    variables: dict[str, str] = {"pcfiledir": path.parent.as_posix()}
    fields: dict[str, str] = {}
    pending = ""

    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue

        if line.endswith("\\"):
            pending += line[:-1]
            continue
        line = pending + line
        pending = ""

        if "=" in line and (":" not in line or line.index("=") < line.index(":")):
            key, value = line.split("=", 1)
            variables[key.strip()] = value.strip()
            continue

        key, value = line.split(":", 1)
        fields[key.strip()] = value.strip()

    expanded_variables = {
        key: expand_value(value, variables) for key, value in variables.items()
    }
    return PcPackage(
        name=path.stem,
        version=expand_value(fields.get("Version", "0"), expanded_variables),
        libs=split_flags(expand_value(fields.get("Libs", ""), expanded_variables)),
        libs_private=split_flags(
            expand_value(fields.get("Libs.private", ""), expanded_variables)
        ),
        cflags=split_flags(expand_value(fields.get("Cflags", ""), expanded_variables)),
        requires=parse_requires(
            expand_value(fields.get("Requires", ""), expanded_variables)
        ),
        requires_private=parse_requires(
            expand_value(fields.get("Requires.private", ""), expanded_variables)
        ),
    )


def expand_value(value: str, variables: dict[str, str]) -> str:
    result = value
    for _ in range(20):
        replaced = VARIABLE_PATTERN.sub(
            lambda match: variables.get(match.group(1), ""), result
        )
        if replaced == result:
            return replaced
        result = replaced
    return result


def parse_requires(raw: str) -> list[PackageSpec]:
    specs: list[PackageSpec] = []
    for part in raw.split(","):
        part = part.strip()
        if not part:
            continue
        specs.append(parse_spec(part))
    return specs


def parse_spec(raw: str) -> PackageSpec:
    match = VERSION_PATTERN.match(raw)
    if not match:
        raise ValueError(f"Unable to parse package spec: {raw}")
    name, op, version = match.groups()
    return PackageSpec(name=name, op=op, version=version or None)


def ensure_version(package: PcPackage, spec: PackageSpec) -> None:
    if spec.op is None or spec.version is None:
        return
    comparison = compare_versions(package.version, spec.version)
    okay = {
        "=": comparison == 0,
        ">=": comparison >= 0,
        "<=": comparison <= 0,
        ">": comparison > 0,
        "<": comparison < 0,
    }[spec.op]
    if not okay:
        raise ValueError(
            f"Package '{spec.name}' version {package.version} does not satisfy {spec.op} {spec.version}"
        )


def compare_versions(left: str, right: str) -> int:
    left_parts = tokenize_version(left)
    right_parts = tokenize_version(right)
    length = max(len(left_parts), len(right_parts))
    for index in range(length):
        left_part = left_parts[index] if index < len(left_parts) else 0
        right_part = right_parts[index] if index < len(right_parts) else 0
        if left_part == right_part:
            continue
        if isinstance(left_part, int) and isinstance(right_part, int):
            return 1 if left_part > right_part else -1
        return 1 if str(left_part) > str(right_part) else -1
    return 0


def tokenize_version(version: str) -> list[int | str]:
    tokens = re.findall(r"\d+|[A-Za-z]+", version)
    parsed: list[int | str] = []
    for token in tokens:
        parsed.append(int(token) if token.isdigit() else token)
    return parsed


def split_flags(raw: str) -> list[str]:
    return [part.strip().strip('"').strip("'") for part in raw.split() if part]


def dedupe(items: list[str]) -> list[str]:
    seen: set[str] = set()
    output: list[str] = []
    for item in items:
        if item in seen:
            continue
        seen.add(item)
        output.append(item)
    return output


def find_pc_file(name: str) -> Path | None:
    for directory in candidate_directories():
        path = directory / f"{name}.pc"
        if path.is_file():
            return path
    return None


def candidate_directories() -> list[Path]:
    directories: list[Path] = []
    for variable in ("PKG_CONFIG_PATH", "PKG_CONFIG_LIBDIR"):
        directories.extend(parse_path_list(os.environ.get(variable, "")))

    if not directories:
        for prefix in candidate_prefixes():
            directories.extend(pkgconfig_dirs_for_prefix(prefix))

    root = active_vcpkg_root()
    for triplet in candidate_triplets():
        base = root / "installed" / triplet
        if is_debug_build():
            directories.append(base / "debug" / "lib" / "pkgconfig")
        directories.append(base / "lib" / "pkgconfig")
        directories.append(base / "share" / "pkgconfig")
        if not is_debug_build():
            directories.append(base / "debug" / "lib" / "pkgconfig")

    return existing_paths(directories)


def parse_path_list(raw: str) -> list[Path]:
    if not raw:
        return []
    if os.name == "nt":
        if ";" in raw:
            parts = raw.split(";")
        elif re.match(r"^[A-Za-z]:[\\/]", raw):
            parts = [raw]
        else:
            parts = raw.split(":")
    else:
        parts = raw.split(":")
    return [normalize_path(part) for part in parts if part]


def normalize_path(raw: str) -> Path:
    text = raw.strip()
    if re.match(r"^/[A-Za-z]/", text):
        drive = text[1].upper()
        text = f"{drive}:{text[2:]}"
    return Path(text.replace("/", os.sep))


if __name__ == "__main__":
    raise SystemExit(main())
