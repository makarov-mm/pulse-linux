cargo = find_program('cargo')

cargo_options = [
  '--manifest-path', meson.project_source_root() / 'Cargo.toml',
  '--target-dir', meson.project_build_root() / 'cargo-target',
]
if get_option('buildtype') == 'release'
  cargo_options += ['--release']
  cargo_profile = 'release'
else
  cargo_profile = 'debug'
endif

# Cargo tracks the sources itself, so the target is always handed to it.
custom_target(
  'cargo-build',
  build_by_default: true,
  build_always_stale: true,
  console: true,
  output: meson.project_name(),
  install: true,
  install_dir: bindir,
  command: [
    'env', 'LOCALEDIR=' + localedir,
    cargo, 'build', cargo_options,
    '&&',
    'cp', 'cargo-target' / cargo_profile / meson.project_name(), '@OUTPUT@',
  ],
)
