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
import { fieldNameFrom, inferFieldType, slugify, uniqueName } from "./intakeDraft";
import { FederationPanel } from "./FederationPanel";
import { federationText } from "./federationMessages";
import "./intake.css";

type SiteRow = IntakeSite & { key: string };
/// 设计器里的字段： `min` / `max` 存用户输入的原文。"-"、"1." 这类中间态不该
/// 被 Number() 吃掉，保存时再转成数字。
type FieldRow = Omit<IntakeField, "min" | "max"> & {
  key: string;
  min: string;
  max: string;
  /// 名字和类型还是跟着显示名走的；用户自己改过就关掉。
  nameAuto: boolean;
  typeAuto: boolean;
};
type FormRow = Omit<IntakeForm, "fields"> & { key: string; slugAuto: boolean; fields: FieldRow[] };
type HookRow = IntakeWebhook & { key: string };
type Draft = { enabled: boolean; listen: string; publicUrl: string; sites: SiteRow[]; forms: FormRow[]; webhooks: HookRow[] };
type Tab = "sites" | "forms" | "webhooks" | "records" | "federation";
type Picker = { target: "site" | "form" | "hook"; rowKey: string; path: string; data: BrowseView | null; error: string };
type Copy = ReturnType<typeof intakeText>;
type Update = (change: (draft: Draft) => Draft) => void;

/// 输入框里空着表示这一端不限，所以下拉里没有「无」这个选项，只有文本输入。
const emptyField = (): FieldRow => ({
  key: crypto.randomUUID(), name: "", label: "", type: "text", required: false,
  options: [], allow_decimal: false, min: "", max: "", nameAuto: true, typeAuto: true,
});

/// 类型下拉按用途分组，选起来比一长串平铺快。
const fieldTypeGroups: Array<{ key: string; label: "typeGroupText" | "typeGroupContact" | "typeGroupOther"; types: IntakeFieldType[] }> = [
  { key: "text", label: "typeGroupText", types: ["text", "textarea"] },
  { key: "contact", label: "typeGroupContact", types: ["email", "tel"] },
  { key: "other", label: "typeGroupOther", types: ["number", "url", "date", "choice"] },
];

/// 与 `crates/willdeep-cli/src/config.rs` 的 `INTAKE_FORM_TOKEN_FIELD` 一致：
/// 这个字段名放令牌，不能给用户字段用。前端先挡，省得保存时才收到英文报错。
const RESERVED_FIELD_NAME = "__willdeep_form_token";

function fromView(view: Pick<IntakeView, "enabled" | "listen" | "public_url" | "sites" | "forms" | "webhooks">): Draft {
  return {
    enabled: view.enabled,
    listen: view.listen,
    publicUrl: view.public_url ?? "",
    sites: view.sites.map((site) => ({ ...site, key: crypto.randomUUID() })),
    forms: view.forms.map((form) => ({
      ...form,
      key: crypto.randomUUID(),
      slugAuto: false,
      fields: form.fields.map((field) => ({
        ...field,
        key: crypto.randomUUID(),
        allow_decimal: field.allow_decimal ?? false,
        min: field.min == null ? "" : String(field.min),
        max: field.max == null ? "" : String(field.max),
        nameAuto: false,
        typeAuto: false,
      })),
    })),
    webhooks: view.webhooks.map((hook) => ({ ...hook, key: crypto.randomUUID() })),
  };
}

/// 输入框原文转数字。空串、写坏的值都当成「不限」，交给前端校验去拦。
function numberOrNull(text: string): number | null {
  const trimmed = text.trim();
  if (!trimmed) return null;
  const value = Number(trimmed);
  return Number.isFinite(value) ? value : null;
}

function payload(draft: Draft): IntakePayload {
  return {
    enabled: draft.enabled,
    listen: draft.listen.trim(),
    public_url: draft.publicUrl.trim(),
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
        allow_decimal: field.type === "number" ? field.allow_decimal : false,
        min: field.type === "number" ? numberOrNull(field.min) : null,
        max: field.type === "number" ? numberOrNull(field.max) : null,
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
      <label className="intake-wide">{t.publicUrl}
        <input value={draft.publicUrl} disabled={!writable} placeholder="https://intake.example.com" onChange={(event) => update((current) => ({ ...current, publicUrl: event.target.value }))} />
        <span className="intake-hint">{t.publicUrlHint}</span>
      </label>
      <div className="intake-listener-actions">
        <button type="button" className="intake-save" disabled={!writable || !dirty || saving} onClick={() => void save()}>
          {saving ? t.saving : t.save}
        </button>
      </div>
    </section>

    <nav className="intake-tabs">
      {(["sites", "forms", "webhooks", "records", "federation"] as const).map((key) => (
        <button key={key} type="button" aria-pressed={tab === key} onClick={() => setTab(key)}>{key === "federation" ? federationText(language).tab : t[key]}</button>
      ))}
    </nav>

    {tab === "sites" && <SitesTab draft={draft} writable={writable} update={update} t={t} view={data} onBrowse={openPicker} />}
    {tab === "forms" && <FormsTab draft={draft} writable={writable} update={update} t={t} view={data} onBrowse={openPicker} />}
    {tab === "webhooks" && <WebhooksTab draft={draft} writable={writable} update={update} t={t} view={data} onBrowse={openPicker} />}
    {tab === "records" && <RecordsTab view={data} language={language} t={t} onOpenSession={onOpenSession} onRefresh={() => void view.refetch()} />}
    {tab === "federation" && <FederationPanel language={language} onOpenSession={onOpenSession} />}

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

  // slug 在站点、表单和 webhook 之间共用一个命名空间，自动命名时要避开别人。
  const takenSlugs = (current: Draft, exceptFormKey: string) => {
    const taken = new Set<string>();
    for (const site of current.sites) if (site.slug.trim()) taken.add(site.slug.trim());
    for (const form of current.forms) if (form.key !== exceptFormKey && form.slug.trim()) taken.add(form.slug.trim());
    for (const hook of current.webhooks) if (hook.slug.trim()) taken.add(hook.slug.trim());
    return taken;
  };

  // 只有还处在自动模式的表单才跟着标题重算 slug；手动改过的不动。
  // 中文标题没有可用的转写，退到 `form` 占位——至少能直接保存，标题下面会说明。
  const autoSlug = (title: string): string => slugify(title) || (title.trim() ? "form" : "");
  const changeTitle = (formKey: string, title: string) =>
    update((current) => ({
      ...current,
      forms: current.forms.map((form) => {
        if (form.key !== formKey) return form;
        if (!form.slugAuto) return { ...form, title };
        const base = autoSlug(title);
        return { ...form, title, slug: base ? uniqueName(base, takenSlugs(current, formKey)) : "" };
      }),
    }));

  // 显示名一改，自动模式的字段名和类型跟着走；手动改过的不动。
  const changeLabel = (formKey: string, fieldKey: string, label: string) =>
    changeForm(formKey, (form) => {
      const taken = new Set(form.fields.filter((field) => field.key !== fieldKey && field.name.trim()).map((field) => field.name.trim()));
      return {
        ...form,
        fields: form.fields.map((field) => {
          if (field.key !== fieldKey) return field;
          const next = { ...field, label };
          if (field.nameAuto) next.name = uniqueName(fieldNameFrom(label), taken);
          if (field.typeAuto) next.type = inferFieldType(label) ?? "text";
          return next;
        }),
      };
    });

  const moveField = (formKey: string, fieldKey: string, delta: number) =>
    changeForm(formKey, (form) => {
      const index = form.fields.findIndex((field) => field.key === fieldKey);
      const target = index + delta;
      if (index < 0 || target < 0 || target >= form.fields.length) return form;
      const fields = [...form.fields];
      [fields[index], fields[target]] = [fields[target], fields[index]];
      return { ...form, fields };
    });

  return <section className="intake-section">
    <p className="intake-hint">{t.formsHint}</p>
    {draft.forms.map((form) => {
      const link = intakeLink(view, "f", form.slug);
      const autoBase = slugify(form.title);
      const slugHint = !form.slugAuto ? t.slugHint : !form.title.trim() ? t.slugAutoEmpty : autoBase ? t.slugAutoHint : t.slugAutoFallback;
      return <div className="intake-card" key={form.key}>
        <div className="intake-card-title">
          <strong>{form.title || form.slug || t.addForm}</strong>
          {link && <a href={link} target="_blank" rel="noreferrer">{`/f/${form.slug}`}</a>}
        </div>
        <label>{t.slug}
          <div className="intake-input-badge">
            <input aria-label={t.slug} value={form.slug} disabled={!writable} placeholder={t.slugPlaceholder} onChange={(event) => {
              const value = event.target.value;
              update((current) => ({
                ...current,
                forms: current.forms.map((row) => {
                  if (row.key !== form.key) return row;
                  if (value.trim()) return { ...row, slug: value, slugAuto: false };
                  // 清空 = 交还给标题：立刻按现在的标题重算，而不是留个空框。
                  const base = autoSlug(row.title);
                  return { ...row, slug: base ? uniqueName(base, takenSlugs(current, row.key)) : "", slugAuto: true };
                }),
              }));
            }} />
            {form.slugAuto && <span className="intake-auto" title={t.slugAutoHint}>{t.auto}</span>}
          </div>
          <span className="intake-hint">{slugHint}</span>
        </label>
        <label>{t.formTitle}
          <input aria-label={t.formTitle} value={form.title} disabled={!writable} onChange={(event) => changeTitle(form.key, event.target.value)} />
        </label>
        <label className="intake-wide">{t.workspace}
          <div className="intake-row">
            <input aria-label={t.workspace} value={form.workspace} disabled={!writable} placeholder={t.rootPlaceholder} onChange={(event) => changeForm(form.key, (row) => ({ ...row, workspace: event.target.value }))} />
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
          <textarea aria-label={t.prompt} rows={2} value={form.prompt} disabled={!writable} onChange={(event) => changeForm(form.key, (row) => ({ ...row, prompt: event.target.value }))} />
          <span className="intake-hint">{t.promptHint}</span>
        </label>
        <div className="intake-fields intake-wide">
          <Text className="intake-fields-title">{t.fields}</Text>
          {form.fields.map((field, index) => <div className="intake-field-row" key={field.key}>
            <div className="intake-field-line">
              <input className="intake-field-label" aria-label={t.fieldLabel} value={field.label} disabled={!writable} placeholder={t.fieldLabelPlaceholder} onChange={(event) => changeLabel(form.key, field.key, event.target.value)} />
              <select className="intake-field-type" aria-label={t.fieldType} value={field.type} disabled={!writable} onChange={(event) => changeField(form.key, field.key, (row) => ({ ...row, type: event.target.value as IntakeFieldType, typeAuto: false }))}>
                {fieldTypeGroups.map((group) => <optgroup key={group.key} label={t[group.label]}>
                  {group.types.map((type) => <option key={type} value={type}>{fieldTypeLabel(type, t)}</option>)}
                </optgroup>)}
              </select>
              <label className="intake-inline"><input type="checkbox" checked={field.required} disabled={!writable} onChange={(event) => changeField(form.key, field.key, (row) => ({ ...row, required: event.target.checked }))} />{t.fieldRequired}</label>
              <div className="intake-field-actions">
                <button type="button" title={t.moveUp} aria-label={t.moveUp} disabled={!writable || index === 0} onClick={() => moveField(form.key, field.key, -1)}>↑</button>
                <button type="button" title={t.moveDown} aria-label={t.moveDown} disabled={!writable || index === form.fields.length - 1} onClick={() => moveField(form.key, field.key, 1)}>↓</button>
                <button type="button" className="intake-danger" title={t.remove} aria-label={t.remove} disabled={!writable} onClick={() => changeForm(form.key, (row) => ({ ...row, fields: row.fields.filter((item) => item.key !== field.key) }))}>×</button>
              </div>
            </div>
            <div className="intake-field-meta">
              <span>{t.fieldName}</span>
              <input className="intake-mono" aria-label={t.fieldName} value={field.name} disabled={!writable} placeholder={t.fieldName} onChange={(event) => {
                const value = event.target.value;
                changeForm(form.key, (row) => {
                  const taken = new Set(row.fields.filter((item) => item.key !== field.key && item.name.trim()).map((item) => item.name.trim()));
                  return {
                    ...row,
                    fields: row.fields.map((item) => {
                      if (item.key !== field.key) return item;
                      // 清空同样交还给显示名，和 slug 的规则一致。
                      return value.trim()
                        ? { ...item, name: value, nameAuto: false }
                        : { ...item, name: uniqueName(fieldNameFrom(item.label), taken), nameAuto: true };
                    }),
                  };
                });
              }} />
              {field.nameAuto && <span className="intake-auto" title={t.nameAutoHint}>{t.auto}</span>}
            </div>
            {field.type === "choice" && <input className="intake-mono" aria-label={t.fieldOptions} value={field.options.join(", ")} disabled={!writable} placeholder={t.fieldOptions} onChange={(event) => changeField(form.key, field.key, (row) => ({ ...row, options: event.target.value.split(",") }))} />}
            {field.type === "number" && <div className="intake-field-extra">
              <label className="intake-inline"><input type="checkbox" checked={field.allow_decimal} disabled={!writable} onChange={(event) => changeField(form.key, field.key, (row) => ({ ...row, allow_decimal: event.target.checked }))} />{t.allowDecimal}</label>
              <label>{t.minValue}<input className="intake-mono" inputMode="decimal" aria-label={t.minValue} value={field.min} disabled={!writable} placeholder={t.noLimit} onChange={(event) => changeField(form.key, field.key, (row) => ({ ...row, min: event.target.value }))} /></label>
              <label>{t.maxValue}<input className="intake-mono" inputMode="decimal" aria-label={t.maxValue} value={field.max} disabled={!writable} placeholder={t.noLimit} onChange={(event) => changeField(form.key, field.key, (row) => ({ ...row, max: event.target.value }))} /></label>
            </div>}
          </div>)}
          <button type="button" className="intake-add" disabled={!writable} onClick={() => changeForm(form.key, (row) => ({ ...row, fields: [...row.fields, emptyField()] }))}>{t.addField}</button>
        </div>
        <div className="intake-card-actions">
          <button type="button" className="intake-danger" disabled={!writable} onClick={() => update((current) => ({ ...current, forms: current.forms.filter((row) => row.key !== form.key) }))}>{t.remove}</button>
        </div>
      </div>;
    })}
    {!draft.forms.length && <p>{t.emptyForms}</p>}
    <button type="button" className="intake-add" disabled={!writable} onClick={() => update((current) => ({ ...current, forms: [...current.forms, { key: crypto.randomUUID(), slug: "", slugAuto: true, title: "", workspace: "", session: "continue", prompt: "", fields: [] }] }))}>{t.addForm}</button>
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
  if (type === "number") return t.typeNumber;
  if (type === "tel") return t.typeTel;
  if (type === "url") return t.typeUrl;
  if (type === "date") return t.typeDate;
  return t.typeText;
}

function parentPath(path: string) {
  const trimmed = path.replace(/\/+$/, "");
  const index = trimmed.lastIndexOf("/");
  return index <= 0 ? "/" : trimmed.slice(0, index);
}

/// 公网地址只能是 origin：带协议和主机，不带路径、查询、片段或凭据。带路径时
/// `/share/...` 拼出来的地址和入口实际挂载的路径对不上，所以前端先挡掉。
/// 这里同样检查原串的结构——`https://x\`、`https://x?` 会被 URL 解析器改写，
/// 原样拼路径同样打不开；大小写、显式默认端口和国际化域名则照常放行。
function validPublicUrl(value: string): boolean {
  const trimmed = value.trim();
  if (!trimmed) return true;
  try {
    const url = new URL(trimmed);
    if (url.protocol !== "http:" && url.protocol !== "https:") return false;
    const schemeEnd = trimmed.indexOf("://");
    if (schemeEnd <= 0) return false;
    const rawAuthority = trimmed.slice(schemeEnd + 3);
    const authority = rawAuthority.endsWith("/") ? rawAuthority.slice(0, -1) : rawAuthority;
    const hasControl = [...authority].some((char) => {
      const code = char.charCodeAt(0);
      return code < 0x20 || code === 0x7f;
    });
    return authority.length > 0
      && url.hostname.length > 0
      && !hasControl
      && !/[/\\?#\s]/.test(authority);
  } catch {
    return false;
  }
}

/// 前端先挡掉后端一定会拒绝的输入，省得用户点了保存才看到一句英文报错。
function validate(draft: Draft, t: Copy): string | null {
  if (!/^[^\s]+:\d+$/.test(draft.listen.trim())) return t.missingField.replace("{field}", t.listen);
  if (!validPublicUrl(draft.publicUrl)) return t.badPublicUrl;
  const groups: Array<[string, Array<{ slug: string }>, string]> = [
    ["site", draft.sites, t.sites],
    ["form", draft.forms, t.forms],
    ["webhook", draft.webhooks, t.webhooks],
  ];
  for (const [kind, list, label] of groups) {
    const seen = new Set<string>();
    for (const row of list) {
      // 先问「填了没有」，再问格式：新建的空表单说「缺标题」比说「slug 格式错」清楚。
      if (kind === "site" && !(row as SiteRow).root.trim()) return t.missingField.replace("{field}", `${label} · ${t.root}`);
      if (kind === "form") {
        const form = row as FormRow;
        if (!form.title.trim()) return t.missingField.replace("{field}", `${label} · ${t.formTitle}`);
        if (!form.workspace.trim()) return t.missingField.replace("{field}", `${label} · ${t.workspace}`);
        if (!form.prompt.trim()) return t.missingField.replace("{field}", `${label} · ${t.prompt}`);
      }
      if (kind === "webhook") {
        const hook = row as HookRow;
        if (!hook.workspace.trim()) return t.missingField.replace("{field}", `${label} · ${t.workspace}`);
        if (!hook.prompt.trim()) return t.missingField.replace("{field}", `${label} · ${t.prompt}`);
        if (!hook.secret_env.trim()) return t.missingField.replace("{field}", `${label} · ${t.secretEnv}`);
      }
      const slug = row.slug.trim();
      if (!/^[A-Za-z0-9-]{1,64}$/.test(slug)) return `${t.badSlug}: ${label} ${slug || "—"}`;
      if (seen.has(slug)) return `${t.duplicateSlug}: ${slug}`;
      seen.add(slug);
      if (kind === "form") {
        const form = row as FormRow;
        const names = new Set<string>();
        for (const field of form.fields) {
          const name = field.name.trim();
          if (!name) return t.missingField.replace("{field}", `${label} · ${t.fieldName}`);
          if (name === RESERVED_FIELD_NAME) return `${label} · ${name} · ${t.reservedFieldName}`;
          if (names.has(name)) return `${t.duplicateSlug}: ${name}`;
          names.add(name);
          if (field.type === "choice" && !field.options.some((option) => option.trim())) return t.missingField.replace("{field}", `${label} · ${name} · ${t.fieldOptions}`);
          if (field.type === "number") {
            const min = numberOrNull(field.min);
            const max = numberOrNull(field.max);
            if (field.min.trim() && min === null) return `${label} · ${name} · ${t.badNumberBound}`;
            if (field.max.trim() && max === null) return `${label} · ${name} · ${t.badNumberBound}`;
            if (min !== null && max !== null && min > max) return `${label} · ${name} · ${t.badNumberRange}`;
          }
        }
      }
    }
  }
  return null;
}
