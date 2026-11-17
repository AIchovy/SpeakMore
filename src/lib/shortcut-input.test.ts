import { it, expect } from 'vitest'
import { shortcutLabels, shortcutDescription } from './shortcut-input'
it('displays physical combinations with readable accessibility labels', () => {
  expect(shortcutLabels('Fn')).toEqual(['Fn'])
  expect(shortcutLabels('Control+Alt+KeyX')).toEqual(['⌃', '⌥', 'X'])
  expect(shortcutDescription('Alt+Super+KeyX')).toBe('Option + Command + X')
})
