import { Fragment, useRef } from "react";
import { Menu, Portal, useMenu } from "@chakra-ui/react";
import { Archive, ArchiveRestore, Ellipsis, GitFork, Pencil, Pin, PinOff, Trash2 } from "lucide-react";
import type { Messages } from "./i18n";
import "./sessionActionMenu.css";

type SessionAction = "pin" | "rename" | "fork" | "archive" | "delete";
const shortcuts = { pin: "P", rename: "R", fork: "F", archive: "A", delete: "D" } satisfies Record<SessionAction, string>;

export function SessionActionMenu({ title, messages, pinned, archived, busy, active, forkDisabled, onAction }: {
  title: string;
  messages: Messages;
  pinned: boolean;
  archived: boolean;
  busy: boolean;
  active: boolean;
  forkDisabled: boolean;
  onAction: (action: SessionAction) => void;
}) {
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menu = useMenu({
    loopFocus: true,
    typeahead: false,
    onEscapeKeyDown: () => {
      // An arrow key may already have scheduled focus on the menu for the next frame.
      requestAnimationFrame(() => triggerRef.current?.focus({ preventScroll: true }));
    },
    positioning: { strategy: "fixed", placement: "bottom-end", gutter: 4, overflowPadding: 8, fitViewport: true, hideWhenDetached: true },
  });
  const editDisabled = busy || active;
  const entries = [
    { value: "pin", label: pinned ? messages.unpinSession : messages.pinSession, icon: pinned ? PinOff : Pin, disabled: busy },
    { value: "rename", label: messages.renameSession, icon: Pencil, disabled: editDisabled },
    { value: "fork", label: messages.forkSession, icon: GitFork, disabled: editDisabled || forkDisabled },
    { value: "archive", label: archived ? messages.unarchiveSession : messages.archiveSession, icon: archived ? ArchiveRestore : Archive, disabled: editDisabled, separate: true },
    { value: "delete", label: messages.deleteSession, icon: Trash2, disabled: editDisabled, separate: true },
  ] as const;

  return (
    <Menu.RootProvider value={menu} lazyMount unmountOnExit>
      <Menu.Trigger ref={triggerRef} className="session-menu-trigger" title={messages.sessionActions} aria-label={messages.sessionActionsFor.replace("{title}", () => title)}>
        <Ellipsis size={17} aria-hidden="true" />
      </Menu.Trigger>
      <Portal>
        <Menu.Positioner>
          <Menu.Content
            className="session-menu"
            onContextMenu={(event) => event.stopPropagation()}
            onKeyDownCapture={(event) => {
              if (event.key !== "Tab" || event.defaultPrevented) return;
              // Resume native tab order from the trigger, not the portalled content.
              event.stopPropagation();
              triggerRef.current?.focus({ preventScroll: true });
              menu.service.send({ type: "CLOSE", restoreFocus: false });
            }}
            onKeyDown={(event) => {
              // The menu itself owns focus (aria-activedescendant). Never handle a key from an input.
              if (event.target !== event.currentTarget || event.defaultPrevented || event.currentTarget.hidden
                || event.ctrlKey || event.metaKey || event.altKey || event.repeat || event.nativeEvent.isComposing) return;
              const entry = entries.find(({ value }) => shortcuts[value] === event.key.toUpperCase());
              if (!entry || entry.disabled) return;
              event.preventDefault();
              event.stopPropagation();
              // Follow the same selection and focus-restoration path as a pointer click.
              event.currentTarget.querySelector<HTMLElement>(`[data-value="${entry.value}"]`)?.click();
            }}
          >
            {entries.map((entry) => {
              const { value, label, icon: Icon, disabled } = entry;
              const shortcut = shortcuts[value];
              return (
                <Fragment key={value}>
                  {"separate" in entry && <Menu.Separator />}
                  <Menu.Item value={value} disabled={disabled} aria-keyshortcuts={disabled ? undefined : shortcut} onClick={() => { if (!disabled) onAction(value); }}>
                    <Icon size={15} aria-hidden="true" />
                    <Menu.ItemText>{label}</Menu.ItemText>
                    <Menu.ItemCommand aria-hidden="true">{shortcut}</Menu.ItemCommand>
                  </Menu.Item>
                </Fragment>
              );
            })}
          </Menu.Content>
        </Menu.Positioner>
      </Portal>
    </Menu.RootProvider>
  );
}
