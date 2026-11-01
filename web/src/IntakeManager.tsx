// 公网入口管理页：站点发布、表单设计、webhook 与会话绑定。
//
// 页面改的就是 `config.toml` 里的 `[intake]` 段。保存后由 Runtime 里的入口服务
// 按新配置重开监听；独立跑 Web 时只落盘，页面会如实提示需要重启。
//
// 密钥不经过这里：webhook 只填「从哪个环境变量读」，密钥本身留在环境里。

import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Button, Dialog, Flex, Portal, Text } from "@chakra-ui/react";
import type { Language } from "./i18n";
import { intakeText } from "./intakeMessages";
import {
  browseDirectory,
  intakeLink,
  IntakeFailure,
  loadIntake,
  saveIntake,
  type BrowseView,
  type IntakeField,
  type IntakeFieldType,
  type IntakeForm,
  type IntakePayload,
  type IntakeSite,
  type IntakeView,
  type IntakeWebhook,
} from "./intakeApi";
import "./intake.css";

type SiteRow = IntakeSite & { key: string };
type FieldRow = IntakeField & { key: string };
type FormRow = Omit<IntakeForm, "fields"> & { key: string; fields: FieldRow[] };
type HookRow = IntakeWebhook & { key: string };
type Draft = { enabled: boolean; listen: string; sites: SiteRow[]; forms: FormRow[]; webhooks: HookRow[] };
type Tab = "sites" | "forms" | "webhooks" | "records";
type Picker = { target: "site" | "form" | "hook"; rowKey: string; path: string; data: BrowseView | null; error: string };
type Copy = ReturnType<typeof intakeText>;
type Update = (change: (draft: Draft) => Draft) => void;

const fieldTypes: IntakeFieldType[] = ["text", "textarea", "email", "choice"];
const emptyField = (): FieldRow => ({ key: crypto.randomUUID(), name: "", label: "", type: "text", required: false, options: [] });

function fromView(view: Pick<IntakeView, "enabled" | "listen" | "sites" | "forms" | "webhooks">): Draft {
  return {
    enabled: view.enabled,
    listen: view.listen,
    sites: view.sites.map((site) => ({ ...site, key: crypto.randomUUID() })),
    forms: view.forms.map((form) => ({
      ...form,
      key: crypto.randomUUID(),
      fields: form.fields.map((field) => ({ ...field, key: crypto.randomUUID() })),
    })),
    webhooks: view.webhooks.map((hook) => ({ ...hook, key: crypto.randomUUID() })),
  };
}

function payload(draft: Draft): IntakePayload {
  return {
    enabled: draft.enabled,
    listen: draft.listen.trim(),
    sites: draft.sites.map(({ slug, root }) => ({ slug: slug.trim(), root: root.trim() })),
    forms: draft.forms.map((form) => ({
      slug: form.slug.trim(),
      title: form.title.trim(),
      workspace: form.workspace.trim(),
      session: form.session,
      prompt: form.prompt,
      fields: form.fields.map((field) => ({
        name: field.name.trim(),
        label: field.label.trim(),
        type: field.type,
        required: field.required,
        options: field.type === "choice" ? field.options.map((option) => option.trim()).filter(Boolean) : [],
      })),
    })),
    webhooks: draft.webhooks.map((hook) => ({
      slug: hook.slug.trim(),
      workspace: hook.workspace.trim(),
      session: hook.session,
      prompt: hook.prompt,
      secret_env: hook.secret_env.trim(),
    })),
  };
}

export function IntakeManager({ language, onOpenSession }: { language: Language; onOpenSession: (sessionId: string) => void }) {
  const t = intakeText(language);
  const client = useQueryClient();
  const view = useQuery({ queryKey: ["intake"], queryFn: ({ signal }) => loadIntake(signal) });
  const [draft, setDraft] = useState<Draft | null>(null);
  const [baseline, setBaseline] = useState("");
  const [tab, setTab] = useState<Tab>("sites");
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState<{ tone: "ok" | "error"; text: string } | null>(null);
  const [picker, setPicker] = useState<Picker | null>(null);

  useEffect(() => {
    if (!view.data || draft !== null) return;
    const next = fromView(view.data);
    setDraft(next);
    setBaseline(JSON.stringify(payload(next)));
  }, [view.data, draft]);

  const update: Update = (change) => setDraft((current) => current ? change(current) : current);
  const dirty = draft !== null && JSON.stringify(payload(draft)) !== baseline;
  const writable = view.data?.writable ?? false;
  const data = view.data ?? null;

  async function save() {
    if (!draft) return;
    const problem = validate(draft, t);
    if (problem) { setNotice({ tone: "error", text: problem }); return; }
    setSaving(true);
    setNotice(null);
    try {
      const next = await saveIntake(payload(draft));
      const normalized = fromView(next);
      client.setQueryData(["intake"], next);
      setDraft(normalized);
      setBaseline(JSON.stringify(payload(normalized)));
      setNotice({
        tone: "ok",
        text: next.runtime_managed ? (next.running ? t.reloaded : t.saved) : t.savedNeedsRestart,
      });
    } catch (reason) {
      const detail = reason instanceof IntakeFailure ? reason.message : String(reason);
      setNotice({ tone: "error", text: `${t.saveFailed}: ${detail}` });
    } finally {
      setSaving(false);
    }
  }

  function openPicker(target: Picker["target"], rowKey: string, path: string) {
    setPicker({ target, rowKey, path, data: null, error: "" });
    void enterPicker(path);
  }

  async function enterPicker(path: string) {
    try {
      const data = await browseDirectory(path);
      setPicker((current) => current && { ...current, path: data.path, data, error: "" });
    } catch (reason) {
      setPicker((current) => current && {
        ...current,
        error: `${t.browseFailed}: ${reason instanceof IntakeFailure ? reason.message : String(reason)}`,
      });
    }
  }

  function choose(directory: string) {
    if (!picker?.data) return;
    const path = `${picker.data.path.replace(/\/$/, "")}/${directory}`;
    update((current) => {
      if (picker.target === "site") return { ...current, sites: current.sites.map((row) => row.key === picker.rowKey ? { ...row, root: path } : row) };
      if (picker.target === "form") return { ...current, forms: current.forms.map((row) => row.key === picker.rowKey ? { ...row, workspace: path } : row) };
      return { ...current, webhooks: current.webhooks.map((row) => row.key === picker.rowKey ? { ...row, workspace: path } : row) };
    });
    setPicker(null);
  }

  if (view.isLoading) return <main className="intake-manager"><Text>{t.loading}</Text></main>;
  if (view.error) return <main className="intake-manager"><Text role="alert">{t.loadFailed}: {view.error.message}</Text></main>;
  if (!draft || !data) return <main className="intake-manager"><Text>{t.loading}</Text></main>;

  return <main className="intake-manager">
    <header>
      <div><h1>{t.title}</h1><p>{t.subtitle}</p></div>
      <div className="intake-header-actions">
        <span className={`intake-status${data.running ? " running" : ""}`}>
          {data.running ? `${t.running}${data.running_listen ? ` · ${data.running_listen}` : ""}` : t.stopped}
        </span>
        <button type="button" className="intake-save" disabled={!writable || !dirty || saving} onClick={() => void save()}>
          {saving ? t.saving : t.save}
        </button>
      </div>
    </header>

    {!writable && <p className="intake-notice">{t.notWritable}</p>}
    {writable && <p className="intake-hint">{t.commentHint}</p>}
    {dirty && <p className="intake-dirty">{t.unsaved}</p>}
    {notice && <p role={notice.tone === "error" ? "alert" : "status"} className={notice.tone === "error" ? "intake-error" : "intake-notice"}>{notice.text}</p>}

    <section className="intake-listener">
      <label className="intake-checkbox">
        <input type="checkbox" checked={draft.enabled} disabled={!writable} onChange={(event) => update((current) => ({ ...current, enabled: event.target.checked }))} />
        {t.enabled}
      </label>
      <label>{t.listen}
        <input value={draft.listen} disabled={!writable} placeholder="127.0.0.1:9857" onChange={(event) => update((current) => ({ ...current, listen: event.target.value }))} />
      </label>
      <p className="intake-hint">{t.listenHint}</p>
    </section>

    <nav className="intake-tabs">
      {(["sites", "forms", "webhooks", "records"] as const).map((key) => (
        <button key={key} type="button" aria-pressed={tab === key} onClick={() => setTab(key)}>{t[key]}</button>
      ))}
    </nav>

    {tab === "sites" && <SitesTab draft={draft} writable={writable} update={update} t={t} view={data} onBrowse={openPicker} />}
    {tab === "forms" && <FormsTab draft={draft} writable={writable} update={update} t={t} view={data} onBrowse={openPicker} />}
    {tab === "webhooks" && <WebhooksTab draft={draft} writable={writable} update={update} t={t} view={data} onBrowse={openPicker} />}
    {tab === "records" && <RecordsTab view={data} language={language} t={t} onOpenSession={onOpenSession} onRefresh={() => void view.refetch()} />}

    <Dialog.Root open={picker !== null} onOpenChange={(details) => { if (!details.open) setPicker(null); }}>
      <Portal>
        <Dialog.Backdrop bg="var(--shadow-strong)" />
        <Dialog.Positioner>
          <Dialog.Content bg="var(--bg-surface)" color="var(--text)" border="1px solid" borderColor="var(--bg-elevated)" borderRadius="12px" maxW="520px" maxH="70vh">
            <Dialog.Header><Dialog.Title>{t.browseTitle}</Dialog.Title></Dialog.Header>
            <Dialog.Body>
              <Text fontSize="xs" color="var(--text-dim)" mb="2" overflowWrap="anywhere">{picker?.path}</Text>
              {picker?.error && <Text role="alert" color="var(--danger-text)" mb="2">{picker.error}</Text>}
              <Flex gap="2" mb="3">
                <Button size="xs" variant="outline" onClick={() => void enterPicker(parentPath(picker?.data?.path ?? picker?.path ?? "/"))}>{t.browseUp}</Button>
              </Flex>
              <div className="intake-directories">
                {picker?.data?.directories.map((directory) => <div className="intake-directory" key={directory}>
                  <button type="button" className="intake-link" onClick={() => void enterPicker(`${picker.data?.path.replace(/\/$/, "")}/${directory}`)}>{directory}/</button>
                  <button type="button" onClick={() => choose(directory)}>{t.browseChoose}</button>
                </div>)}
                {picker?.data && !picker.data.directories.length && <Text fontSize="sm" color="var(--text-dim)">{t.browseEmpty}</Text>}
              </div>
            </Dialog.Body>
          </Dialog.Content>
        </Dialog.Positioner>
      </Portal>
    </Dialog.Root>
  </main>;
}

function SitesTab({ draft, writable, update, t, view, onBrowse }: {
  draft: Draft; writable: boolean; update: Update; t: Copy; view: IntakeView; onBrowse: (target: Picker["target"], rowKey: string, path: string) => void;
}) {
  return <section className="intake-section">
    <p className="intake-hint">{t.sitesHint}</p>
    {draft.sites.map((site) => {
      const link = intakeLink(view, "s", site.slug);
      return <div className="intake-card" key={site.key}>
        <label>{t.slug}
          <input value={site.slug} disabled={!writable} placeholder="docs" onChange={(event) => update((current) => ({ ...current, sites: current.sites.map((row) => row.key === site.key ? { ...row, slug: event.target.value } : row) }))} />
          <span className="intake-hint">{t.slugHint}</span>
        </label>
        <label className="intake-wide">{t.root}
          <div className="intake-row">
            <input value={site.root} disabled={!writable} placeholder={t.rootPlaceholder} onChange={(event) => update((current) => ({ ...current, sites: current.sites.map((row) => row.key === site.key ? { ...row, root: event.target.value } : row) }))} />
            <button type="button" disabled={!writable} onClick={() => onBrowse("site", site.key, site.root)}>{t.browse}</button>
          </div>
        </label>
        <div className="intake-card-actions">
          {link && <a href={link} target="_blank" rel="noreferrer">{t.openSite}</a>}
          <button type="button" className="intake-danger" disabled={!writable} onClick={() => update((current) => ({ ...current, sites: current.sites.filter((row) => row.key !== site.key) }))}>{t.remove}</button>
        </div>
      </div>;
    })}
    {!draft.sites.length && <p>{t.emptySites}</p>}
    <button type="button" className="intake-add" disabled={!writable} onClick={() => update((current) => ({ ...current, sites: [...current.sites, { key: crypto.randomUUID(), slug: "", root: "" }] }))}>{t.addSite}</button>
  </section>;
}

function FormsTab({ draft, writable, update, t, view, onBrowse }: {
  draft: Draft; writable: boolean; update: Update; t: Copy; view: IntakeView; onBrowse: (target: Picker["target"], rowKey: string, path: string) => void;
}) {
  const changeForm = (key: string, change: (form: FormRow) => FormRow) =>
    update((current) => ({ ...current, forms: current.forms.map((form) => form.key === key ? change(form) : form) }));
  const changeField = (formKey: string, fieldKey: string, change: (field: FieldRow) => FieldRow) =>
    changeForm(formKey, (form) => ({ ...form, fields: form.fields.map((field) => field.key === fieldKey ? change(field) : field) }));
  return <section className="intake-section">
    <p className="intake-hint">{t.formsHint}</p>
    {draft.forms.map((form) => <div className="intake-card" key={form.key}>
      <div className="intake-card-title">
        <strong>{form.title || form.slug || t.addForm}</strong>
        {intakeLink(view, "f", form.slug) && <a href={intakeLink(view, "f", form.slug) ?? ""} target="_blank" rel="noreferrer">{`/f/${form.slug}`}</a>}
      </div>
      <label>{t.slug}<input value={form.slug} disabled={!writable} onChange={(event) => changeForm(form.key, (row) => ({ ...row, slug: event.target.value }))} /></label>
      <label>{t.formTitle}<input value={form.title} disabled={!writable} onChange={(event) => changeForm(form.key, (row) => ({ ...row, title: event.target.value }))} /></label>
      <label className="intake-wide">{t.workspace}
        <div className="intake-row">
          <input value={form.workspace} disabled={!writable} placeholder={t.rootPlaceholder} onChange={(event) => changeForm(form.key, (row) => ({ ...row, workspace: event.target.value }))} />
          <button type="button" disabled={!writable} onClick={() => onBrowse("form", form.key, form.workspace)}>{t.browse}</button>
        </div>
      </label>
      <label>{t.session}
        <select value={form.session} disabled={!writable} onChange={(event) => changeForm(form.key, (row) => ({ ...row, session: event.target.value as IntakeForm["session"] }))}>
          <option value="continue">{t.sessionContinue}</option>
          <option value="new">{t.sessionNew}</option>
        </select>
      </label>
      <label className="intake-wide">{t.prompt}
        <textarea rows={2} value={form.prompt} disabled={!writable} onChange={(event) => changeForm(form.key, (row) => ({ ...row, prompt: event.target.value }))} />
        <span className="intake-hint">{t.promptHint}</span>
      </label>
      <div className="intake-fields intake-wide">
        <Text className="intake-fields-title">{t.fields}</Text>
        {form.fields.map((field) => <div className="intake-field-row" key={field.key}>
          <input aria-label={t.fieldName} value={field.name} disabled={!writable} placeholder={t.fieldName} onChange={(event) => changeField(form.key, field.key, (row) => ({ ...row, name: event.target.value }))} />
          <input aria-label={t.fieldLabel} value={field.label} disabled={!writable} placeholder={t.fieldLabel} onChange={(event) => changeField(form.key, field.key, (row) => ({ ...row, label: event.target.value }))} />
          <select aria-label={t.fieldType} value={field.type} disabled={!writable} onChange={(event) => changeField(form.key, field.key, (row) => ({ ...row, type: event.target.value as IntakeFieldType }))}>
            {fieldTypes.map((type) => <option key={type} value={type}>{fieldTypeLabel(type, t)}</option>)}
          </select>
          <label className="intake-inline"><input type="checkbox" checked={field.required} disabled={!writable} onChange={(event) => changeField(form.key, field.key, (row) => ({ ...row, required: event.target.checked }))} />{t.fieldRequired}</label>
          {field.type === "choice" && <input className="intake-wide" aria-label={t.fieldOptions} value={field.options.join(", ")} disabled={!writable} placeholder={t.fieldOptions} onChange={(event) => changeField(form.key, field.key, (row) => ({ ...row, options: event.target.value.split(",").map((option) => option.trim()) }))} />}
          <button type="button" className="intake-danger" disabled={!writable} onClick={() => changeForm(form.key, (row) => ({ ...row, fields: row.fields.filter((item) => item.key !== field.key) }))}>{t.remove}</button>
        </div>)}
        <button type="button" className="intake-add" disabled={!writable} onClick={() => changeForm(form.key, (row) => ({ ...row, fields: [...row.fields, emptyField()] }))}>{t.addField}</button>
      </div>
      <div className="intake-card-actions">
        <button type="button" className="intake-danger" disabled={!writable} onClick={() => update((current) => ({ ...current, forms: current.forms.filter((row) => row.key !== form.key) }))}>{t.remove}</button>
      </div>
    </div>)}
    {!draft.forms.length && <p>{t.emptyForms}</p>}
    <button type="button" className="intake-add" disabled={!writable} onClick={() => update((current) => ({ ...current, forms: [...current.forms, { key: crypto.randomUUID(), slug: "", title: "", workspace: "", session: "continue", prompt: "", fields: [] }] }))}>{t.addForm}</button>
  </section>;
}

function WebhooksTab({ draft, writable, update, t, view, onBrowse }: {
  draft: Draft; writable: boolean; update: Update; t: Copy; view: IntakeView; onBrowse: (target: Picker["target"], rowKey: string, path: string) => void;
}) {
  const change = (key: string, patch: (hook: HookRow) => HookRow) =>
    update((current) => ({ ...current, webhooks: current.webhooks.map((hook) => hook.key === key ? patch(hook) : hook) }));
  return <section className="intake-section">
    <p className="intake-hint">{t.webhooksHint}</p>
    {draft.webhooks.map((hook) => <div className="intake-card" key={hook.key}>
      <div className="intake-card-title">
        <strong>{hook.slug || t.addWebhook}</strong>
        {intakeLink(view, "w", hook.slug) && <Text as="code">{`/w/${hook.slug}`}</Text>}
      </div>
      <label>{t.slug}<input value={hook.slug} disabled={!writable} onChange={(event) => change(hook.key, (row) => ({ ...row, slug: event.target.value }))} /></label>
      <label className="intake-wide">{t.workspace}
        <div className="intake-row">
          <input value={hook.workspace} disabled={!writable} placeholder={t.rootPlaceholder} onChange={(event) => change(hook.key, (row) => ({ ...row, workspace: event.target.value }))} />
          <button type="button" disabled={!writable} onClick={() => onBrowse("hook", hook.key, hook.workspace)}>{t.browse}</button>
        </div>
      </label>
      <label>{t.session}
        <select value={hook.session} disabled={!writable} onChange={(event) => change(hook.key, (row) => ({ ...row, session: event.target.value as IntakeWebhook["session"] }))}>
          <option value="continue">{t.sessionContinue}</option>
          <option value="new">{t.sessionNew}</option>
        </select>
      </label>
      <label>{t.secretEnv}
        <input value={hook.secret_env} disabled={!writable} placeholder="INTAKE_SECRET" onChange={(event) => change(hook.key, (row) => ({ ...row, secret_env: event.target.value }))} />
        <span className="intake-hint">{t.secretEnvHint}</span>
      </label>
      <label className="intake-wide">{t.prompt}
        <textarea rows={2} value={hook.prompt} disabled={!writable} onChange={(event) => change(hook.key, (row) => ({ ...row, prompt: event.target.value }))} />
      </label>
      <div className="intake-card-actions">
        <button type="button" className="intake-danger" disabled={!writable} onClick={() => update((current) => ({ ...current, webhooks: current.webhooks.filter((row) => row.key !== hook.key) }))}>{t.remove}</button>
      </div>
    </div>)}
    {!draft.webhooks.length && <p>{t.emptyWebhooks}</p>}
    <button type="button" className="intake-add" disabled={!writable} onClick={() => update((current) => ({ ...current, webhooks: [...current.webhooks, { key: crypto.randomUUID(), slug: "", workspace: "", session: "continue", prompt: "", secret_env: "" }] }))}>{t.addWebhook}</button>
  </section>;
}

function RecordsTab({ view, language, t, onOpenSession, onRefresh }: {
  view: IntakeView; language: Language; t: Copy; onOpenSession: (sessionId: string) => void; onRefresh: () => void;
}) {
  return <section className="intake-section">
    <p className="intake-hint">{t.recordsHint}</p>
    <div className="intake-section-heading"><h2>{t.bindings}</h2><Button size="xs" variant="outline" onClick={onRefresh}>{t.refresh}</Button></div>
    {view.bindings.length ? <table className="intake-table">
      <thead><tr><th>{t.slug}</th><th>{t.session}</th></tr></thead>
      <tbody>{view.bindings.map((binding) => <tr key={`${binding.slug}-${binding.session_id}`}>
        <td><code>{binding.slug}</code></td>
        <td><button type="button" className="intake-link" onClick={() => onOpenSession(binding.session_id)}>{t.sessionLink}</button> <span className="intake-hint">{binding.session_id.slice(0, 8)}</span></td>
      </tr>)}</tbody>
    </table> : <p>{t.noBindings}</p>}
    <h2>{t.pending}</h2>
    {view.pending.length ? <table className="intake-table">
      <thead><tr><th>{t.failureAt}</th><th>{t.failureSlug}</th><th>{t.failureReason}</th></tr></thead>
      <tbody>{view.pending.map((record, index) => <tr key={`${record.at}-${record.slug}-${index}`}>
        <td>{new Date(record.at * 1000).toLocaleString(language)}</td>
        <td><code>{record.kind}/{record.slug}</code></td>
        <td className="intake-reason">{record.reason}</td>
      </tr>)}</tbody>
    </table> : <p>{t.noPending}</p>}
  </section>;
}

function fieldTypeLabel(type: IntakeFieldType, t: Copy) {
  if (type === "textarea") return t.typeTextarea;
  if (type === "email") return t.typeEmail;
  if (type === "choice") return t.typeChoice;
  return t.typeText;
}

function parentPath(path: string) {
  const trimmed = path.replace(/\/+$/, "");
  const index = trimmed.lastIndexOf("/");
  return index <= 0 ? "/" : trimmed.slice(0, index);
}

/// 前端先挡掉后端一定会拒绝的输入，省得用户点了保存才看到一句英文报错。
function validate(draft: Draft, t: Copy): string | null {
  if (!/^[^\s]+:\d+$/.test(draft.listen.trim())) return t.missingField.replace("{field}", t.listen);
  const groups: Array<[string, Array<{ slug: string }>, string]> = [
    ["site", draft.sites, t.sites],
    ["form", draft.forms, t.forms],
    ["webhook", draft.webhooks, t.webhooks],
  ];
  for (const [kind, list, label] of groups) {
    const seen = new Set<string>();
    for (const row of list) {
      const slug = row.slug.trim();
      if (!/^[A-Za-z0-9-]{1,64}$/.test(slug)) return `${t.badSlug}: ${label} ${slug || "—"}`;
      if (seen.has(slug)) return `${t.duplicateSlug}: ${slug}`;
      seen.add(slug);
      if (kind === "site" && !(row as SiteRow).root.trim()) return t.missingField.replace("{field}", `${label} · ${t.root}`);
      if (kind === "form") {
        const form = row as FormRow;
        if (!form.title.trim()) return t.missingField.replace("{field}", `${label} · ${t.formTitle}`);
        if (!form.workspace.trim()) return t.missingField.replace("{field}", `${label} · ${t.workspace}`);
        if (!form.prompt.trim()) return t.missingField.replace("{field}", `${label} · ${t.prompt}`);
        const names = new Set<string>();
        for (const field of form.fields) {
          const name = field.name.trim();
          if (!name) return t.missingField.replace("{field}", `${label} · ${t.fieldName}`);
          if (names.has(name)) return `${t.duplicateSlug}: ${name}`;
          names.add(name);
          if (field.type === "choice" && !field.options.some((option) => option.trim())) return t.missingField.replace("{field}", `${label} · ${name} · ${t.fieldOptions}`);
        }
      }
      if (kind === "webhook") {
        const hook = row as HookRow;
        if (!hook.workspace.trim()) return t.missingField.replace("{field}", `${label} · ${t.workspace}`);
        if (!hook.prompt.trim()) return t.missingField.replace("{field}", `${label} · ${t.prompt}`);
        if (!hook.secret_env.trim()) return t.missingField.replace("{field}", `${label} · ${t.secretEnv}`);
      }
    }
  }
  return null;
}
