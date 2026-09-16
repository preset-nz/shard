import { ChevronDown, ChevronRight, Plus } from 'lucide-react';
import { useEffect, useState } from 'react';
import {
  addMap,
  type ControllersView,
  type ControlView,
  controllers,
  type DeviceView,
  forgetControl,
  forgetDevice,
  type MappingsView,
  renameBank,
  renameControl,
  renameDevice,
  renameMap,
  setActiveMap,
  setControlMode,
  setControlRole,
} from '@/audio';
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from '@/components/ui/context-menu';
import { Input } from '@/components/ui/input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';

/**
 * Settings → Controllers. Every MIDI port that has been seen, and every knob
 * or pad that has been touched, listed as it was learned. Nothing is mapped
 * here; this is where a control gets its name and where you check that it
 * is arriving at all: the dot beside a control lights while it moves.
 *
 * Right-click a device or a control to forget it. Stories 1 and 2 of
 * `controller-mapping.md`.
 *
 * **Devices fold.** Two controllers is the ordinary case once a second one
 * arrives, and a 24-knob device would otherwise bury an 8-knob one. A group
 * opens when its device is connected and closes when it is not, until you
 * say otherwise; its header carries the state, the control count, and a dot
 * that lights when anything inside it moves, so you can tell which box a
 * knob is on without unfolding either.
 *
 * **Controls sit under their channel.** A channel is how these boxes spell a
 * bank — the LPD8's four programs come out on channels 1, 2, 1 and 4, and the
 * Launch Control's templates do the same — so the channel is the only
 * evidence a bank was switched. Name it ("Mixer", "Program 1") and forty
 * controls read as a few short sections here and in the row menu.
 *
 * A knob also says whether it is a pot or an endless encoder. That is never
 * inferred, because a right turn in one encoding is a left turn in another;
 * set it and turn the encoder one click anticlockwise, and the change beside
 * it reads −1 when the encoding is right.
 *
 * Above the devices sits the map: which of the named, app-wide maps the
 * knobs play through. Switching arms every knob again. Maps are app-wide and
 * a control is addressed by its device, so a second controller learns its own
 * controls and takes nothing from the first. Only a *target* is exclusive:
 * learning a knob onto a parameter another knob already reaches moves it, and
 * says so.
 */
export function ControllersPanel({
  open,
  midi,
  onError,
}: {
  open: boolean;
  midi: MappingsView;
  onError: (message: string) => void;
}) {
  const [view, setView] = useState<ControllersView | null>(null);
  /** Folds you set by hand, by port. Unset means "follow the connection". */
  const [folds, setFolds] = useState<Record<string, boolean>>({});

  // Poll only while visible. Ten times a second is enough for a dot.
  useEffect(() => {
    if (!open) return;
    let alive = true;
    const tick = () =>
      void controllers()
        .then((v) => {
          if (alive) setView(v);
        })
        .catch(() => {});
    tick();
    const id = setInterval(tick, 100);
    return () => {
      alive = false;
      clearInterval(id);
    };
  }, [open]);

  const apply = (p: Promise<ControllersView>) =>
    void p.then(setView).catch((e) => onError(String(e)));

  const fail = (e: unknown) => onError(String(e));

  if (!view) return null;

  const mapRow = (
    <div className="flex items-center gap-2 px-3">
      <span className="shrink-0 text-xs text-muted-foreground">Map</span>
      {midi.active ? (
        <>
          <Select
            value={String(midi.active.id)}
            onValueChange={(id) => void setActiveMap(Number(id)).catch(fail)}
          >
            <SelectTrigger size="sm" className="h-6 w-36 text-xs">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {midi.maps.map((m) => (
                <SelectItem key={m.id} value={String(m.id)} className="text-xs">
                  {m.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <NameField
            value={midi.active.name}
            onCommit={(n) => void renameMap(midi.active?.id ?? 0, n).catch(fail)}
          />
        </>
      ) : (
        <span className="flex-1 text-xs text-muted-foreground">
          None yet. Learn a control from a row and one is made.
        </span>
      )}
      <button
        type="button"
        className="rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground"
        title="New map"
        onClick={() => void addMap(`map ${midi.maps.length + 1}`).catch(fail)}
      >
        <Plus className="size-3.5" />
      </button>
    </div>
  );

  // Starting with no controllers looks like the app forgot them on purpose,
  // so say what actually happened to the file.
  const troubleRow = view.trouble ? (
    <p className="mx-3 rounded border border-amber-500/40 bg-amber-500/10 px-2 py-1.5 text-xs text-amber-500">
      {view.trouble}
    </p>
  ) : null;

  return (
    <div className="space-y-3">
      {mapRow}
      {troubleRow}
      {view.devices.length === 0 ? (
        <p className="px-3 py-2 text-xs text-muted-foreground">
          No MIDI device seen yet. Plug one in and it appears here.
        </p>
      ) : (
        view.devices.map((d) => (
          <DeviceGroup
            key={d.port}
            device={d}
            controls={view.controls.filter((c) => c.device === d.port)}
            roles={view.roles}
            modes={view.modes}
            open={folds[d.port] ?? d.connected}
            onFold={(o) => setFolds((f) => ({ ...f, [d.port]: o }))}
            apply={apply}
          />
        ))
      )}
    </div>
  );
}

/** One controller: a header that reads folded, and its controls. */
function DeviceGroup({
  device: d,
  controls,
  roles,
  modes,
  open,
  onFold,
  apply,
}: {
  device: DeviceView;
  controls: ControlView[];
  roles: string[];
  modes: string[];
  open: boolean;
  onFold: (open: boolean) => void;
  apply: (p: Promise<ControllersView>) => void;
}) {
  const moving = controls.some((c) => c.active);
  const Chevron = open ? ChevronDown : ChevronRight;
  return (
    <div>
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <div className="flex items-center gap-1.5 px-3">
            <button
              type="button"
              className="rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground"
              title={open ? 'Fold' : 'Unfold'}
              aria-expanded={open}
              onClick={() => onFold(!open)}
            >
              <Chevron className="size-3.5" />
            </button>
            <span
              className={
                moving
                  ? 'size-1.5 shrink-0 rounded-full bg-amber-400'
                  : d.connected
                    ? 'size-1.5 shrink-0 rounded-full bg-emerald-400'
                    : 'size-1.5 shrink-0 rounded-full bg-muted-foreground/40'
              }
              title={d.connected ? 'Connected' : 'Not connected'}
            />
            <NameField value={d.name} onCommit={(n) => apply(renameDevice(d.port, n))} />
            <span className="shrink-0 font-mono text-[10px] text-muted-foreground">{d.port}</span>
            <span
              className={
                d.connected
                  ? 'w-28 shrink-0 text-right text-[10px] text-emerald-400'
                  : 'w-28 shrink-0 text-right text-[10px] text-muted-foreground'
              }
            >
              {d.connected ? 'Connected' : 'Not connected'}
            </span>
            <span className="w-16 shrink-0 text-right font-mono text-[10px] tabular-nums text-muted-foreground">
              {controls.length === 1 ? '1 control' : `${controls.length} controls`}
            </span>
          </div>
        </ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuItem onSelect={() => apply(forgetDevice(d.port))}>
            Forget device and its controls
          </ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>
      {!open ? null : controls.length === 0 ? (
        <p className="px-3 py-1 pl-9 text-xs text-muted-foreground">
          Touch a knob or pad and it appears here.
        </p>
      ) : (
        channelsOf(controls).map(([channel, inChannel], _i, all) => (
          <ChannelGroup
            key={channel}
            port={d.port}
            channel={channel}
            name={inChannel[0].bank}
            sole={all.length === 1}
            controls={inChannel}
            roles={roles}
            modes={modes}
            apply={apply}
          />
        ))
      )}
    </div>
  );
}

/** The channels a device has been heard on, in order, with their controls. */
function channelsOf(controls: ControlView[]): [number, ControlView[]][] {
  const by = new Map<number, ControlView[]>();
  for (const c of controls) {
    const got = by.get(c.channel);
    if (got) got.push(c);
    else by.set(c.channel, [c]);
  }
  return [...by.entries()].sort((a, b) => a[0] - b[0]);
}

/**
 * One channel of one device: a bank, in hardware terms. The header is only
 * drawn when the device speaks on more than one, because a box that sits on
 * channel 1 forever does not need a heading that says so.
 */
function ChannelGroup({
  port,
  channel,
  name,
  sole,
  controls,
  roles,
  modes,
  apply,
}: {
  port: string;
  channel: number;
  name: string;
  /** The device's only channel, so its heading would say nothing. */
  sole: boolean;
  controls: ControlView[];
  roles: string[];
  modes: string[];
  apply: (p: Promise<ControllersView>) => void;
}) {
  return (
    <div className="mt-1">
      {sole ? null : (
        <div className="flex items-center gap-2 py-0.5 pr-3 pl-9">
          <NameField
            value={name}
            className="h-5 flex-1 border-transparent px-1.5 text-[10px] tracking-wide text-muted-foreground uppercase hover:border-border"
            onCommit={(n) => apply(renameBank(port, channel, n))}
          />
          <span className="shrink-0 font-mono text-[10px] text-muted-foreground/60">
            ch {channel + 1}
          </span>
        </div>
      )}
      {controls.map((c) => (
        <ContextMenu key={c.id}>
          <ContextMenuTrigger asChild>
            <div className={`flex items-center gap-2 py-0.5 pr-3 ${sole ? 'pl-9' : 'pl-12'}`}>
              <span
                className={
                  c.active
                    ? 'size-1.5 shrink-0 rounded-full bg-amber-400'
                    : 'size-1.5 shrink-0 rounded-full bg-muted-foreground/25'
                }
              />
              <NameField value={c.name} onCommit={(n) => apply(renameControl(c.id, n))} />
              <Select value={c.role} onValueChange={(r) => apply(setControlRole(c.id, r))}>
                <SelectTrigger size="sm" className="h-6 w-16 text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {roles.map((r) => (
                    <SelectItem key={r} value={r} className="text-xs">
                      {r}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {c.role === 'knob' ? (
                <Select value={c.mode} onValueChange={(m) => apply(setControlMode(c.id, m))}>
                  <SelectTrigger
                    size="sm"
                    className="h-6 w-32 text-xs"
                    title="A pot sends where it is; an endless encoder sends how far it moved. Pick an encoding, then turn it one click anticlockwise: the change on the right reads −1 when it is the right one."
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {modes.map((m) => (
                      <SelectItem key={m} value={m} className="text-xs">
                        {m}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              ) : (
                <span className="w-32 shrink-0" />
              )}
              <span className="w-14 shrink-0 text-right font-mono text-[10px] text-muted-foreground">
                {c.kind} {c.number}
              </span>
              <span className="w-10 shrink-0 text-right font-mono text-[10px] tabular-nums text-muted-foreground">
                {c.delta !== null
                  ? `${c.delta > 0 ? '+' : c.delta < 0 ? '\u2212' : ''}${Math.abs(c.delta)}`
                  : (c.last ?? '')}
              </span>
            </div>
          </ContextMenuTrigger>
          <ContextMenuContent>
            <ContextMenuItem onSelect={() => apply(forgetControl(c.id))}>
              Forget control
            </ContextMenuItem>
          </ContextMenuContent>
        </ContextMenu>
      ))}
    </div>
  );
}

/** A name that commits on blur or Enter, and reverts on Escape. */
function NameField({
  value,
  onCommit,
  className,
}: {
  value: string;
  onCommit: (name: string) => void;
  className?: string;
}) {
  const [draft, setDraft] = useState(value);
  const [editing, setEditing] = useState(false);
  // While not editing, follow the value from Rust.
  useEffect(() => {
    if (!editing) setDraft(value);
  }, [value, editing]);
  return (
    <Input
      className={className ?? 'h-6 flex-1 px-1.5 text-xs'}
      value={draft}
      onFocus={() => setEditing(true)}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={() => {
        setEditing(false);
        if (draft.trim() !== value) onCommit(draft);
      }}
      onKeyDown={(e) => {
        if (e.key === 'Enter') (e.target as HTMLInputElement).blur();
        if (e.key === 'Escape') {
          setDraft(value);
          setEditing(false);
          (e.target as HTMLInputElement).blur();
        }
      }}
    />
  );
}
