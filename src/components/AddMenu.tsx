import { Icons } from '@preset.nz/ux-kit';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';

export interface AddChoice<K extends string> {
  kind: K;
  label: string;
  /** Why it cannot be added, shown greyed out. */
  disabled?: string;
}

/**
 * A tree category's + button: one dropdown of the kinds it can add, rather
 * than a button per kind (Georg, 2026-09-27).
 */
export function AddMenu<K extends string>({
  title,
  choices,
  onAdd,
}: {
  /** "Add a generator", read as the button's tooltip and the menu's heading. */
  title: string;
  choices: AddChoice<K>[];
  onAdd: (kind: K) => void;
}) {
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger
        render={
          <button
            type="button"
            title={title}
            // Keep focus off the button, so Space still starts playback.
            onMouseDown={(e) => e.preventDefault()}
            className="rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground"
          />
        }
      >
        <Icons.PlusIcon size={13} />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-44">
        <DropdownMenuGroup>
          <DropdownMenuLabel>{title} of type</DropdownMenuLabel>
        </DropdownMenuGroup>
        {choices.map((c) => (
          <DropdownMenuItem
            key={c.kind}
            disabled={c.disabled !== undefined}
            onSelect={() => onAdd(c.kind)}
            className="text-xs"
          >
            <span className="flex-1">{c.label}</span>
            {c.disabled && <span className="text-[10px] text-muted-foreground">{c.disabled}</span>}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
