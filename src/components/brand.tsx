import type { SVGProps } from 'react'

const voicePath =
  'M205 32 C118 32 56 97 56 179 L56 310 C56 349 75 372 99 384 C113 391 127 385 127 370 C126 302 150 267 215 249 L259 237 C313 222 337 196 337 147 C337 81 287 32 221 32 Z'

export function SpeakMoreMark(props: SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 512 512" fill="currentColor" aria-hidden="true" focusable="false" {...props}>
      <path d={voicePath} />
      <path d={voicePath} transform="rotate(180 256 256)" />
    </svg>
  )
}
