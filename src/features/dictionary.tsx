import { asAppError, type UiMessage } from '@/lib/errors'
import { useRef, useState, type FormEvent } from 'react'
import { useApp, useText } from '@/lib/app-state'
import { type DictionaryEntry, type DictionaryInput } from '@/lib/types'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import { Badge } from '@/components/ui/badge'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { PageTitle, EmptyState, Choice, InlineError, Confirm } from '@/components/common'
import { DictionaryIcon, PlusIcon, SearchIcon, EditIcon, TrashIcon } from '@/components/icons'

export function Dictionary() {
  const editorTrigger = useRef<HTMLElement | null>(null)
  const addTrigger = useRef<HTMLButtonElement | null>(null)
  const { data, mutate, pending } = useApp(),
    t = useText()
  const [query, setQuery] = useState(''),
    [editor, setEditor] = useState<DictionaryEntry | 'new' | null>(null),
    [deleting, setDeleting] = useState<DictionaryEntry | null>(null)
  const [draft, setDraft] = useState<DictionaryInput>({
      term: '',
      locale: 'all',
      enabled: true,
      pinned: false,
    }),
    [error, setError] = useState<UiMessage | null>(null)
  if (!data) return null
  const entries = data.dictionary
    .filter((e) => e.term.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()))
    .sort(
      (a, b) =>
        Number(b.enabled) - Number(a.enabled) ||
        Number(b.pinned) - Number(a.pinned) ||
        a.term.localeCompare(b.term),
    )
  function edit(entry: DictionaryEntry | 'new', trigger: HTMLElement) {
    editorTrigger.current = trigger
    setDraft(
      entry === 'new'
        ? { term: '', locale: 'all', enabled: true, pinned: false }
        : {
            entryId: entry.entryId,
            term: entry.term,
            locale: entry.locale,
            enabled: entry.enabled,
            pinned: entry.pinned,
          },
    )
    setError(null)
    setEditor(entry)
  }
  function closeEditor() {
    setEditor(null)
    setError(null)
  }
  async function submit(e: FormEvent) {
    e.preventDefault()
    setError(null)
    try {
      await mutate('upsert_dictionary', { expectedRevision: data!.settings.revision, input: draft })
      closeEditor()
    } catch (e) {
      setError(asAppError(e))
    }
  }
  async function toggle(entry: DictionaryEntry, enabled: boolean) {
    try {
      await mutate('upsert_dictionary', {
        expectedRevision: data!.settings.revision,
        input: {
          entryId: entry.entryId,
          term: entry.term,
          locale: entry.locale,
          pinned: entry.pinned,
          enabled,
        },
      })
    } catch (e) {
      setError(asAppError(e))
    }
  }
  return (
    <>
      <PageTitle
        title={t('你的词典', 'Your dictionary')}
        description={t(
          '帮助识别人名、产品名和常用术语，不会强制替换原文。',
          'Guide recognition of names, products, and common terms without forcing replacements.',
        )}
        action={
          <Button
            ref={addTrigger}
            onClick={(event) => edit('new', event.currentTarget)}
            disabled={data.dictionary.length >= 200}
          >
            <PlusIcon />
            {t('添加词条', 'Add word')}
          </Button>
        }
      />
      <div className="mb-5 flex flex-wrap items-center justify-between gap-4">
        <div className="relative w-full sm:max-w-[300px]">
          <SearchIcon className="absolute top-2.5 left-3 size-4 text-muted-foreground" />
          <Input
            aria-label={t('搜索词条', 'Search dictionary')}
            placeholder={t('搜索词条…', 'Search words…')}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            className="border-border bg-card pl-9"
          />
        </div>
        <span className="text-xs text-muted-foreground">
          {data.dictionary.length} / 200 {t('个词条', 'words')}
        </span>
      </div>
      {!editor && <InlineError message={error} />}
      <div className="overflow-hidden rounded-xl border bg-card">
        {entries.length === 0 ? (
          <EmptyState
            icon={DictionaryIcon}
            title={
              query
                ? t('没有找到匹配词条', 'No matching words')
                : t('把熟悉的词，介绍给 SpeakMore', 'Introduce your vocabulary')
            }
            description={
              query
                ? t('试试其他关键词，或清除搜索条件。', 'Try another search or clear your filter.')
                : t(
                    '添加你常说的人名、品牌和专业词汇。词典仅用于本地识别提示，不会被整本上传。',
                    'Add names, brands, and specialist terms. Dictionary hints are processed locally, never uploaded as a collection.',
                  )
            }
            action={
              <Button
                variant="outline"
                onClick={(event) => (query ? setQuery('') : edit('new', event.currentTarget))}
              >
                {query ? t('清除搜索', 'Clear search') : t('添加第一个词条', 'Add your first word')}
              </Button>
            }
          />
        ) : (
          <Table>
            <TableHeader>
              <TableRow className="bg-muted/40">
                <TableHead className="pl-6">{t('词条', 'Word')}</TableHead>
                {<TableHead>{t('语言', 'Language')}</TableHead>}
                <TableHead>{t('启用', 'Enabled')}</TableHead>
                <TableHead className="w-24">
                  <span className="sr-only">{t('操作', 'Actions')}</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {entries.map((entry) => (
                <TableRow key={entry.entryId}>
                  <TableCell className="max-w-72 py-5 pl-6 font-medium">
                    <span className="break-words">{entry.term}</span>
                    {entry.pinned && (
                      <Badge variant="secondary" className="ml-2 font-normal">
                        {t('优先', 'Priority')}
                      </Badge>
                    )}
                  </TableCell>
                  <TableCell className="text-muted-foreground">
                    {entry.locale === 'all'
                      ? t('全部', 'All')
                      : entry.locale === 'zh'
                        ? t('中文', 'Chinese')
                        : t('英文', 'English')}
                  </TableCell>
                  <TableCell>
                    <Switch
                      aria-label={`${t('启用', 'Enable')} ${entry.term}`}
                      checked={entry.enabled}
                      onCheckedChange={(v) => void toggle(entry, v)}
                      disabled={pending}
                    />
                  </TableCell>
                  <TableCell className="pr-4">
                    <div className="flex">
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        aria-label={`${t('编辑', 'Edit')} ${entry.term}`}
                        onClick={(event) => edit(entry, event.currentTarget)}
                      >
                        <EditIcon />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        aria-label={`${t('删除', 'Delete')} ${entry.term}`}
                        onClick={() => setDeleting(entry)}
                      >
                        <TrashIcon />
                      </Button>
                    </div>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </div>
      <Dialog
        open={editor !== null}
        onOpenChange={(v) => {
          if (!v && !pending) closeEditor()
        }}
      >
        <DialogContent
          className="sm:max-w-[440px]"
          closeLabel={t('关闭', 'Close')}
          onCloseAutoFocus={(event) => {
            event.preventDefault()
            const target = editorTrigger.current?.isConnected
              ? editorTrigger.current
              : addTrigger.current
            target?.focus()
          }}
        >
          <form onSubmit={(e) => void submit(e)}>
            <DialogHeader>
              <DialogTitle>
                {editor === 'new' ? t('添加词条', 'Add word') : t('编辑词条', 'Edit word')}
              </DialogTitle>
              <DialogDescription>
                {t(
                  '保持自然写法，最多 40 个字符。',
                  'Use its natural spelling, up to 40 characters.',
                )}
              </DialogDescription>
            </DialogHeader>
            <div className="space-y-5 py-6">
              <div className="space-y-3">
                <Label htmlFor="term">{t('词条', 'Word')}</Label>
                <Input
                  id="term"
                  autoFocus
                  value={draft.term}
                  placeholder={t('例如：SpeakMore', 'For example: SpeakMore')}
                  onChange={(e) => setDraft({ ...draft, term: e.target.value })}
                  aria-invalid={Boolean(error)}
                  required
                />
              </div>
              <div className="space-y-3">
                <Label>{t('适用语言', 'Language')}</Label>
                <Choice
                  label={t('词条语言', 'Word language')}
                  value={draft.locale}
                  onChange={(v) => setDraft({ ...draft, locale: v as DictionaryInput['locale'] })}
                  options={[
                    ['all', t('全部语言', 'All languages')],
                    ['zh', t('中文', 'Chinese')],
                    ['en', t('英文', 'English')],
                  ]}
                  className="w-full"
                />
              </div>
              <div className="flex items-center justify-between">
                <Label htmlFor="pin-term">{t('优先使用这个词条', 'Prioritize this word')}</Label>
                <Switch
                  id="pin-term"
                  aria-describedby="pin-term-description"
                  checked={draft.pinned}
                  onCheckedChange={(v) => setDraft({ ...draft, pinned: v })}
                />
              </div>
              <p id="pin-term-description" className="text-xs leading-5 text-muted-foreground">
                {t(
                  '优先使用置顶和最近编辑的词条；每次可用数量取决于模型。',
                  'Pinned and recently edited words take priority, within the model’s limit.',
                )}
              </p>
              <InlineError message={error} />
            </div>
            <DialogFooter>
              <Button type="button" variant="outline" onClick={closeEditor} disabled={pending}>
                {t('取消', 'Cancel')}
              </Button>
              <Button type="submit" disabled={pending}>
                {pending ? t('正在保存…', 'Saving…') : t('保存词条', 'Save word')}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
      <Confirm
        open={deleting !== null}
        onOpenChange={(v) => {
          if (!v) setDeleting(null)
        }}
        title={t('删除这个词条？', 'Delete this word?')}
        description={`${deleting?.term ?? ''} ${t('将不再作为本地识别提示。', 'will no longer be used as a local recognition hint.')}`}
        onConfirm={() =>
          mutate('delete_dictionary', {
            expectedRevision: data.settings.revision,
            entryId: deleting!.entryId,
          })
        }
      />
    </>
  )
}
