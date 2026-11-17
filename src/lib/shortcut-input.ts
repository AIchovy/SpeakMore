export function shortcutLabels(accelerator: string) {
  const labels: Record<string, string> = {
    Control: '⌃',
    Alt: '⌥',
    Shift: '⇧',
    Super: '⌘',
    Escape: 'Esc',
    Space: 'Space',
    Enter: 'Return',
    Backspace: '⌫',
    Delete: 'Delete',
    ArrowUp: '↑',
    ArrowDown: '↓',
    ArrowLeft: '←',
    ArrowRight: '→',
  }
  return accelerator.split('+').map((key) => labels[key] ?? key.replace(/^(Key|Digit)/, ''))
}

export function shortcutDescription(accelerator: string) {
  return accelerator
    .split('+')
    .map((key) =>
      key === 'Alt' ? 'Option' : key === 'Super' ? 'Command' : key.replace(/^(Key|Digit)/, ''),
    )
    .join(' + ')
}
