import { PropertyPanel } from '@preset.nz/facets';
import { useEffect, useId, useState } from 'react';
import type { MappingsView } from '@/audio';
import { ControllersPanel } from '@/components/ControllersPanel';
import type { ParamRowContext } from '@/components/ParamRow';
import { Checkbox } from '@/components/ui/checkbox';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import {
  type AgentSettings,
  agentModels,
  agentServers,
  agentSettings,
  type KnownServer,
  type ModelEntry,
  setAgentEnabled,
  setAgentSettings,
} from '@/lib/agent';
import { AUTO, modelFromPick, OTHER, serverFromPick, serverShown } from '@/lib/modelPick';

import { type ParamValues, scopeKeyFor } from '@/scope';

/**
 * Where the agent's model is. Read when the dialog opens, saved when a field
 * is left (or Enter is pressed); Rust cleans the values and says what it kept.
 * The model is a dropdown of what the server lists, read again whenever the
 * address is saved; a server that lists nothing gets a typed name instead.
 */
function AgentFields({ open, onError }: { open: boolean; onError: (message: string) => void }) {
  const [saved, setSaved] = useState<AgentSettings | null>(null);
  const [baseUrl, setBaseUrl] = useState('');
  const [model, setModel] = useState('');
  const [apiKey, setApiKey] = useState('');
  // Null until listed, or when nothing answers at the address.
  const [models, setModels] = useState<ModelEntry[] | null>(null);
  const [servers, setServers] = useState<KnownServer[]>([]);
  // Other… picked: the URL input shows even while the saved URL is a known one.
  const [otherPicked, setOtherPicked] = useState(false);
  const urlId = useId();
  const modelId = useId();
  const keyId = useId();

  useEffect(() => {
    if (!open) return;
    setOtherPicked(false);
    agentServers().then(setServers, (e) => onError(String(e)));
    agentSettings()
      .then((s) => {
        setSaved(s);
        setBaseUrl(s.baseUrl);
        setModel(s.model);
        setApiKey(s.apiKey);
      })
      .catch((e) => onError(String(e)));
  }, [open, onError]);

  const listedAt = saved?.baseUrl;
  const listedWith = saved?.apiKey;
  useEffect(() => {
    if (!open || listedAt === undefined) return;
    let current = true;
    setModels(null);
    agentModels(listedAt, listedWith ?? '').then(
      (m) => current && setModels(m),
      () => current && setModels(null),
    );
    return () => {
      current = false;
    };
  }, [open, listedAt, listedWith]);

  const save = (next: AgentSettings) => {
    if (
      !saved ||
      (next.baseUrl === saved.baseUrl && next.model === saved.model && next.apiKey === saved.apiKey)
    )
      return;
    setAgentSettings(next)
      .then((s) => {
        setSaved(s);
        setBaseUrl(s.baseUrl);
        setModel(s.model);
        setApiKey(s.apiKey);
      })
      .catch((e) => onError(String(e)));
  };
  const showUrl =
    saved !== null &&
    (otherPicked ||
      serverShown(
        saved.baseUrl,
        servers.map((k) => k.baseUrl),
      ) === OTHER);
  const commit = () => save({ baseUrl, model, apiKey });
  const onEnter = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter') commit();
  };

  return (
    <div className="mt-1 space-y-2 px-3">
      <div className="text-xs">
        <label htmlFor={urlId} className="text-muted-foreground">
          Server
        </label>
        <ServerSelect
          id={urlId}
          servers={servers}
          baseUrl={saved?.baseUrl ?? baseUrl}
          other={otherPicked}
          disabled={!saved}
          onPick={(pick) => {
            if ('other' in pick) {
              setOtherPicked(true);
              return;
            }
            setOtherPicked(false);
            setBaseUrl(pick.save);
            save({ baseUrl: pick.save, model, apiKey });
          }}
        />
        {showUrl && (
          <Input
            aria-label="Server address"
            className="mt-1"
            value={baseUrl}
            onChange={(e) => setBaseUrl(e.target.value)}
            onBlur={commit}
            onKeyDown={onEnter}
            disabled={!saved}
            spellCheck={false}
          />
        )}
      </div>
      <div className="text-xs">
        <label htmlFor={keyId} className="text-muted-foreground">
          API key
        </label>
        <Input
          id={keyId}
          type="password"
          value={apiKey}
          placeholder="Only if the server asks for one"
          onChange={(e) => setApiKey(e.target.value)}
          onBlur={commit}
          onKeyDown={onEnter}
          disabled={!saved}
          autoComplete="off"
          spellCheck={false}
        />
      </div>
      <div className="text-xs">
        <label htmlFor={modelId} className="text-muted-foreground">
          Model
        </label>
        {models?.length ? (
          <ModelSelect
            id={modelId}
            models={models}
            value={model}
            onPick={(m) => {
              setModel(m);
              save({
                baseUrl: saved?.baseUrl ?? baseUrl,
                model: m,
                apiKey: saved?.apiKey ?? apiKey,
              });
            }}
          />
        ) : (
          <Input
            id={modelId}
            value={model}
            placeholder="Whatever is loaded"
            onChange={(e) => setModel(e.target.value)}
            onBlur={commit}
            onKeyDown={onEnter}
            disabled={!saved}
            spellCheck={false}
          />
        )}
      </div>
      <p className="text-xs text-muted-foreground">
        Sampling: Qwen&apos;s recommended values (edit agent.json to change them)
      </p>
    </div>
  );
}

/**
 * The server: one item per server the kit knows, plus Other…. A saved URL no
 * known server has selects Other…. The list comes from Rust.
 */
function ServerSelect({
  id,
  servers,
  baseUrl,
  other,
  disabled,
  onPick,
}: {
  id: string;
  servers: KnownServer[];
  baseUrl: string;
  other: boolean;
  disabled: boolean;
  onPick: (pick: { save: string } | { other: true }) => void;
}) {
  const items: Record<string, string> = {
    ...Object.fromEntries(servers.map((k) => [k.baseUrl, `${k.name} (${k.address})`])),
    [OTHER]: 'Other…',
  };
  const shown = other
    ? OTHER
    : serverShown(
        baseUrl,
        servers.map((k) => k.baseUrl),
      );
  return (
    <Select
      items={items}
      value={shown}
      disabled={disabled || servers.length === 0}
      onValueChange={(v) => {
        const pick = serverFromPick(v, shown);
        if (pick) onPick(pick);
      }}
    >
      <SelectTrigger id={id} size="sm" className="w-full text-xs">
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {Object.entries(items).map(([v, l]) => (
          <SelectItem key={v} value={v} className="text-xs">
            {l}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

/**
 * The model, from the server's list. A model that isn't loaded can be picked,
 * but a reply asks the server only for a loaded one and says so otherwise:
 * LM Studio would load a named model, 20 GB or more, behind your back.
 */
function ModelSelect({
  id,
  models,
  value,
  onPick,
}: {
  id: string;
  models: ModelEntry[];
  value: string;
  onPick: (model: string) => void;
}) {
  const label = (m: ModelEntry) => (m.loaded === false ? `${m.id} (not loaded)` : m.id);
  const items: Record<string, string> = {
    [AUTO]: 'Whatever is loaded',
    ...Object.fromEntries(models.map((m) => [m.id, label(m)])),
  };
  const current = value.trim() === '' || value.trim().toLowerCase() === AUTO ? AUTO : value;
  // A saved model the server no longer lists still shows, rather than a blank.
  if (!(current in items)) items[current] = `${current} (not listed)`;
  return (
    <Select
      items={items}
      value={current}
      onValueChange={(v) => {
        const next = modelFromPick(v, value);
        if (next !== null) onPick(next);
      }}
    >
      <SelectTrigger id={id} size="sm" className="w-full text-xs">
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {Object.entries(items).map(([v, l]) => (
          <SelectItem key={v} value={v} className="text-xs">
            {l}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

/**
 * Settings, on Cmd+,. Today it holds the tape's feel: how long the brake and a
 * reverse flick take (Georg, 2026-09-14). Brake and reverse themselves stay
 * on the header, because they are played.
 *
 * The values are still the patch's own. Roadmap row 4 makes them app-wide
 * defaults a patch can override, once the shared preferences store exists.
 *
 * Controllers are app-wide already: the MIDI devices seen and the controls
 * learned from them, in `controllers.json`.
 */
export function SettingsDialog({
  open,
  onOpenChange,
  values,
  ctx,
  midi,
  onError,
  agentOn,
  onAgentOn,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  values: ParamValues;
  ctx: ParamRowContext;
  midi: MappingsView;
  onError: (message: string) => void;
  /** Labs: the agent switch, as Rust holds it. */
  agentOn: boolean;
  onAgentOn: (on: boolean) => void;
}) {
  const agentSwitchId = useId();
  const switchAgent = (next: boolean) => {
    setAgentEnabled(next).then(onAgentOn, (e) => onError(String(e)));
  };
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Settings</DialogTitle>
          <DialogDescription>
            Tape feel is saved with the patch for now. Controllers and the agent's model belong to
            this Mac.
          </DialogDescription>
        </DialogHeader>
        <div className="-mx-3">
          <div className="px-3 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Tape
          </div>
          <PropertyPanel scopeKey={scopeKeyFor('tape')} selection={values} ctx={ctx} />
          <div className="mt-4 px-3 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Controllers
          </div>
          <div className="mt-1">
            <ControllersPanel open={open} midi={midi} onError={onError} />
          </div>
          <div className="mt-4 px-3 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Labs
          </div>
          <div className="mt-1 flex items-start gap-2 px-3 text-xs">
            <Checkbox
              id={agentSwitchId}
              className="mt-0.5"
              checked={agentOn}
              onCheckedChange={(v) => switchAgent(v === true)}
            />
            <div>
              <label htmlFor={agentSwitchId}>Enable Shard agent</label>
              <p className="text-muted-foreground">
                A local model writes patterns into the session. Needs a model server such as oMLX or
                LM Studio.
              </p>
            </div>
          </div>
          {agentOn && <AgentFields open={open} onError={onError} />}
        </div>
      </DialogContent>
    </Dialog>
  );
}
