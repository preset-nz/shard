import type React from 'react';

/**
 * A track's M or S switch. Lit while pressed; while the press waits for the
 * next bar (pressed and heard differ) it blinks with a ring, and pressing
 * again before the bar cancels it.
 */
export function MixSwitch({
  letter,
  verb,
  name,
  on,
  heard,
  chord,
  onMouseDown,
  onToggle,
}: {
  letter: string;
  /** "Mute" or "Solo". */
  verb: string;
  /** The track's name: "Track 2", "Drums 1". */
  name: string;
  /** As pressed. */
  on: boolean;
  /** As the engine applies it. */
  heard: boolean;
  /** The menu's chord, for the tooltip. */
  chord: string;
  onMouseDown?: (e: React.MouseEvent) => void;
  onToggle: () => void;
}) {
  const waiting = on !== heard;
  const title = waiting
    ? `${name} ${on ? `${verb.toLowerCase()}s` : `stops ${verb === 'Mute' ? 'muting' : 'soloing'}`} on the next bar. Click again to cancel. (${chord})`
    : `${on ? `Un${verb.toLowerCase()}` : verb} ${name} (on the next bar, ${chord})`;
  return (
    <button
      type="button"
      aria-pressed={on}
      aria-label={`${verb} ${name}`}
      title={title}
      // Keep focus off the button, so Space still starts playback.
      onMouseDown={onMouseDown ?? ((e) => e.preventDefault())}
      onClick={(e) => {
        // Not a click on the row or card around it.
        e.stopPropagation();
        onToggle();
      }}
      className={`flex h-3.5 w-3.5 shrink-0 items-center justify-center rounded-sm text-[9px] font-semibold leading-none ${
        on
          ? 'bg-primary text-primary-foreground'
          : 'text-muted-foreground hover:bg-accent hover:text-foreground'
      } ${waiting ? 'animate-pulse ring-1 ring-primary' : ''}`}
    >
      {letter}
    </button>
  );
}
