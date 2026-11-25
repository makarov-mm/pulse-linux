# The Rust sources are read with xgettext's C rules, which is enough for calls with string literals.
i18n.gettext(
  meson.project_name(),
  preset: 'glib',
  args: [
    '--from-code=UTF-8',
    '--add-comments=Translators',
    '--keyword=tr',
    '--keyword=trf',
    '--keyword=trn:1,2',
    '--keyword=mark',
  ],
)
