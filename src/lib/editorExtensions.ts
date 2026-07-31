import { TaskItem, TaskList } from "@tiptap/extension-list";
import { Placeholder } from "@tiptap/extension-placeholder";
import { Extension, type Editor, type JSONContent } from "@tiptap/core";
import { Markdown } from "@tiptap/markdown";
import {
  Fragment,
  type Node as ProseMirrorNode,
  type NodeType,
  type ResolvedPos,
} from "@tiptap/pm/model";
import {
  AllSelection,
  Plugin,
  Selection,
  TextSelection,
  type EditorState,
  type Transaction,
} from "@tiptap/pm/state";
import { canJoin } from "@tiptap/pm/transform";
import { StarterKit } from "@tiptap/starter-kit";
import { search } from "prosemirror-search";

const structuralMarkdownLine =
  /^(?: {0,3}#{1,6}[ \t]+| {0,3}>[ \t]?| {0,3}(?:[-+*]|\d+[.)])[ \t]+| {0,3}(?:`{3,}|~{3,}).*$| {0,3}([-*_])(?:[ \t]*\1){2,}[ \t]*$)/m;
const markdownLink = /!?\[[^\]\n]+\]\(\s*\S+(?:\s+["'][^"'\n]*["'])?\s*\)/;
const inlineCode = /(^|[^`])`[^`\n]+`([^`]|$)/;
const strongEmphasis =
  /(^|[\s([{])(?:\*\*(?=\S)(?:[^*\n]*\S)?\*\*|__(?=\S)(?:[^_\n]*\S)?__)(?=$|[\s)\]},.!?;:])/;
const singleEmphasis =
  /(^|[\s([{])(?:\*(?=\S)(?:[^*\n]*\S)?\*|_(?=\S)(?:[^_\n]*\S)?_)(?=$|[\s)\]},.!?;:])/;

export function looksLikeMarkdown(text: string): boolean {
  if (!text.trim()) return false;
  return (
    structuralMarkdownLine.test(text) ||
    markdownLink.test(text) ||
    inlineCode.test(text) ||
    strongEmphasis.test(text) ||
    singleEmphasis.test(text)
  );
}

export function insertMarkdownPaste(editor: Editor, markdown: string): boolean {
  const manager = editor.markdown;
  if (!manager) return false;
  const parsed = manager.parse(markdown);
  const content = parsed.content ?? [];
  if (content.length === 0) return false;

  const { from, to } = editor.state.selection;
  return editor.commands.insertContentAt({ from, to }, content);
}

export function insertMarkdownClipboard(
  editor: Editor,
  clipboard: Pick<DataTransfer, "getData">,
): boolean {
  const text = clipboard.getData("text/plain");
  if (!looksLikeMarkdown(text)) return false;

  try {
    return insertMarkdownPaste(editor, text);
  } catch (error) {
    console.warn("Could not parse pasted Markdown", error);
    return false;
  }
}

const MarkdownPaste = Extension.create({
  name: "stickyMarkdownPaste",
  priority: 1_100,
  addProseMirrorPlugins() {
    return [
      new Plugin({
        props: {
          handlePaste: (_view, event) => {
            const clipboard = event.clipboardData;
            return clipboard
              ? insertMarkdownClipboard(this.editor, clipboard)
              : false;
          },
        },
      }),
    ];
  },
});

const NoteSearch = Extension.create({
  name: "stickyNoteSearch",
  addProseMirrorPlugins() {
    return [search()];
  },
});

// A task item counts as selected only when the selection touches its own text.
// Testing the item's whole range would also match ancestors of a nested item,
// so a cursor in a sub-task would drag its parent along.
export function selectedTaskItemPositions(state: EditorState): number[] {
  const { from, to } = state.selection;
  const positions: number[] = [];
  state.doc.nodesBetween(from, to, (node, pos) => {
    if (node.type.name !== "taskItem") return;

    const paragraph = node.firstChild;
    if (!paragraph) return;
    const paragraphFrom = pos + 1;
    const paragraphTo = paragraphFrom + paragraph.nodeSize;
    if (from <= paragraphTo && to >= paragraphFrom) positions.push(pos);
  });
  return positions;
}

// Clicking a checkbox only toggles its own item, so a multi-item selection would
// otherwise need one click per box. Apply the clicked box's new state to every
// selected task item instead.
const TaskSelectionCheck = Extension.create({
  name: "stickyTaskSelectionCheck",
  addProseMirrorPlugins() {
    return [
      new Plugin({
        props: {
          handleDOMEvents: {
            // Runs once Tiptap's own handler has toggled the clicked item, so
            // the box the user pressed keeps the state the browser gave it and
            // only the rest of the selection still needs updating. Cancelling
            // the click instead would leave that box visually unchecked: the
            // browser reverts a cancelled checkbox after every click listener,
            // undoing whatever the node view had just written to it.
            change: (view, event) => {
              const target = event.target;
              if (
                !(target instanceof HTMLInputElement) ||
                target.type !== "checkbox"
              ) {
                return false;
              }

              const positions = selectedTaskItemPositions(view.state);
              if (positions.length < 2) return false;

              const clicked = positions.find((pos) => {
                const dom = view.nodeDOM(pos);
                return dom instanceof HTMLElement && dom.contains(target);
              });
              if (clicked === undefined) return false;

              const checked = view.state.doc.nodeAt(clicked)?.attrs.checked;
              const transaction = view.state.tr;
              positions.forEach((pos) => {
                const node = transaction.doc.nodeAt(pos);
                if (!node || node.attrs.checked === checked) return;
                transaction.setNodeMarkup(pos, undefined, {
                  ...node.attrs,
                  checked,
                });
              });
              if (!transaction.docChanged) return false;

              view.dispatch(transaction);
              return true;
            },
          },
        },
      }),
    ];
  },
});

function removeCheckedTasks(nodes: JSONContent[]): {
  nodes: JSONContent[];
  removed: boolean;
} {
  let removed = false;
  const remaining = nodes.flatMap((node) => {
    if (node.type === "taskItem" && node.attrs?.checked === true) {
      removed = true;
      return [];
    }

    const children = removeCheckedTasks(node.content ?? []);
    removed ||= children.removed;

    if (node.type === "taskList" && children.nodes.length === 0) {
      removed = true;
      return [];
    }

    return [
      node.content === undefined ? node : { ...node, content: children.nodes },
    ];
  });

  return { nodes: remaining, removed };
}

function activeListItemType(
  editor: Editor,
): "listItem" | "taskItem" | undefined {
  const { $from } = editor.state.selection;
  for (let depth = $from.depth; depth > 0; depth -= 1) {
    const type = $from.node(depth).type.name;
    if (type === "listItem" || type === "taskItem") return type;
  }
  return undefined;
}

const topLevelListTypes = new Set([
  "bulletList",
  "orderedList",
  "taskList",
]);

function exitTrailingNestedEmptyListItem(
  editor: Editor,
  itemDepth: number,
): boolean {
  const { $from } = editor.state.selection;
  let topLevelListDepth: number | undefined;

  for (let depth = 1; depth < itemDepth; depth += 1) {
    if (
      $from.node(depth - 1).type.name === "doc" &&
      topLevelListTypes.has($from.node(depth).type.name)
    ) {
      topLevelListDepth = depth;
      break;
    }
  }

  if (
    topLevelListDepth === undefined ||
    itemDepth === topLevelListDepth + 1
  ) {
    return false;
  }

  for (let depth = topLevelListDepth; depth <= itemDepth; depth += 1) {
    if ($from.index(depth) !== $from.node(depth).childCount - 1) {
      return false;
    }
  }

  const listDepth = itemDepth - 1;
  const deleteDepth =
    $from.node(listDepth).childCount === 1 ? listDepth : itemDepth;
  const deleteStart = $from.before(deleteDepth);
  const deleteEnd = $from.after(deleteDepth);
  const topLevelListEnd = $from.after(topLevelListDepth);
  const paragraph = editor.schema.nodes.paragraph;
  if (!paragraph) return false;

  return editor.commands.command(({ tr }) => {
    tr.delete(deleteStart, deleteEnd);
    const paragraphPosition = tr.mapping.map(topLevelListEnd);
    tr.insert(paragraphPosition, paragraph.create());
    tr.setSelection(
      TextSelection.create(tr.doc, paragraphPosition + 1),
    );
    tr.scrollIntoView();
    return true;
  });
}

export function liftEmptyListItemOnly(editor: Editor): boolean {
  const { selection } = editor.state;
  const { $from } = selection;
  if (
    !selection.empty ||
    $from.parent.type.name !== "paragraph" ||
    $from.parent.content.size !== 0
  ) {
    return false;
  }

  for (let depth = $from.depth - 1; depth > 0; depth -= 1) {
    const item = $from.node(depth);
    if (item.type.name !== "listItem" && item.type.name !== "taskItem") {
      continue;
    }
    if (
      item.childCount !== 1 ||
      item.firstChild !== $from.parent ||
      $from.index(depth) !== 0
    ) {
      return false;
    }
    if (exitTrailingNestedEmptyListItem(editor, depth)) return true;
    return editor.commands.liftListItem(item.type.name);
  }

  return false;
}

export function deleteEmptyParagraphAfterList(
  state: EditorState,
  dispatch: ((transaction: Transaction) => void) | undefined,
): boolean {
  const { selection } = state;
  const { $from } = selection;
  if (
    !selection.empty ||
    $from.depth !== 1 ||
    $from.parent.type.name !== "paragraph" ||
    $from.parent.content.size !== 0
  ) {
    return false;
  }

  const paragraphIndex = $from.index(0);
  if (
    paragraphIndex === 0 ||
    !topLevelListTypes.has(state.doc.child(paragraphIndex - 1).type.name)
  ) {
    return false;
  }

  const previousText = Selection.findFrom(
    state.doc.resolve($from.before()),
    -1,
    true,
  );
  if (!(previousText instanceof TextSelection)) return false;
  if (!dispatch) return true;

  const transaction = state.tr.delete($from.before(), $from.after());
  transaction.setSelection(
    TextSelection.create(transaction.doc, previousText.from),
  );
  dispatch(transaction.scrollIntoView());
  return true;
}

type ConvertibleListType = "bulletList" | "taskList";

function convertibleListItemAt(
  $position: ResolvedPos,
): ProseMirrorNode | undefined {
  for (let depth = $position.depth; depth > 1; depth -= 1) {
    const item = $position.node(depth);
    const list = $position.node(depth - 1);
    if (
      (item.type.name === "listItem" &&
        list.type.name === "bulletList") ||
      (item.type.name === "taskItem" &&
        list.type.name === "taskList")
    ) {
      return item;
    }
  }
  return undefined;
}

function selectedConvertibleItems(
  state: EditorState,
): Set<ProseMirrorNode> {
  const { doc, selection } = state;
  const selected = new Set<ProseMirrorNode>();

  if (selection.empty) {
    const item = convertibleListItemAt(selection.$from);
    if (item) selected.add(item);
    return selected;
  }

  doc.descendants((node, position) => {
    if (!node.isTextblock) return true;

    const contentStart = position + 1;
    const contentEnd = contentStart + node.content.size;
    const touches =
      node.content.size === 0
        ? selection.from <= contentStart && selection.to > contentStart
        : selection.from < contentEnd && selection.to > contentStart;
    if (!touches) return false;

    const item = convertibleListItemAt(
      doc.resolve(Math.min(contentStart, doc.content.size)),
    );
    if (item) selected.add(item);
    return false;
  });

  return selected;
}

function sameChildren(
  node: ProseMirrorNode,
  children: ProseMirrorNode[],
): boolean {
  return (
    node.childCount === children.length &&
    children.every((child, index) => node.child(index) === child)
  );
}

function transformListNode(
  node: ProseMirrorNode,
  selected: Set<ProseMirrorNode>,
  targetListName: ConvertibleListType,
  targetListType: NodeType,
  targetItemType: NodeType,
): ProseMirrorNode[] {
  const output: ProseMirrorNode[] = [];
  let segmentType: NodeType | undefined;
  let segmentAttrs: ProseMirrorNode["attrs"] | undefined;
  let segmentItems: ProseMirrorNode[] = [];

  const flushSegment = () => {
    if (!segmentType || segmentItems.length === 0) return;
    output.push(
      segmentType.create(
        segmentAttrs,
        Fragment.fromArray(segmentItems),
      ),
    );
    segmentType = undefined;
    segmentAttrs = undefined;
    segmentItems = [];
  };

  node.forEach((item) => {
    const transformedChildren = transformChildren(
      item,
      selected,
      targetListName,
      targetListType,
      targetItemType,
    );
    const isSelected = selected.has(item);

    const desiredListType = isSelected ? targetListType : node.type;
    const alreadyTargetType = isSelected && node.type.name === targetListName;
    const desiredItem = alreadyTargetType
      ? sameChildren(item, transformedChildren)
        ? item
        : item.copy(Fragment.fromArray(transformedChildren))
      : isSelected
        ? targetItemType.create(
            null,
            Fragment.fromArray(transformedChildren),
            item.marks,
          )
        : sameChildren(item, transformedChildren)
          ? item
          : item.copy(Fragment.fromArray(transformedChildren));

    if (segmentType !== desiredListType) {
      flushSegment();
      segmentType = desiredListType;
      segmentAttrs = desiredListType === node.type ? node.attrs : undefined;
    }
    segmentItems.push(desiredItem);
  });

  flushSegment();
  return output;
}

function transformChildren(
  node: ProseMirrorNode,
  selected: Set<ProseMirrorNode>,
  targetListName: ConvertibleListType,
  targetListType: NodeType,
  targetItemType: NodeType,
): ProseMirrorNode[] {
  const children: ProseMirrorNode[] = [];
  node.forEach((child) => {
    children.push(
      ...transformNode(
        child,
        selected,
        targetListName,
        targetListType,
        targetItemType,
      ),
    );
  });
  return children;
}

function transformNode(
  node: ProseMirrorNode,
  selected: Set<ProseMirrorNode>,
  targetListName: ConvertibleListType,
  targetListType: NodeType,
  targetItemType: NodeType,
): ProseMirrorNode[] {
  if (
    node.type.name === "bulletList" ||
    node.type.name === "taskList"
  ) {
    return transformListNode(
      node,
      selected,
      targetListName,
      targetListType,
      targetItemType,
    );
  }
  if (node.isTextblock || node.isLeaf) return [node];

  const children = transformChildren(
    node,
    selected,
    targetListName,
    targetListType,
    targetItemType,
  );
  return [
    sameChildren(node, children)
      ? node
      : node.copy(Fragment.fromArray(children)),
  ];
}

function mappedTextPosition(
  doc: ProseMirrorNode,
  originalParent: ProseMirrorNode,
  parentOffset: number,
): number | undefined {
  let mapped: number | undefined;
  doc.descendants((node, position) => {
    if (node !== originalParent) return mapped === undefined;
    mapped = position + 1 + parentOffset;
    return false;
  });
  return mapped;
}

export function transformSelectedListItems(
  state: EditorState,
  dispatch: ((transaction: Transaction) => void) | undefined,
  targetListName: ConvertibleListType,
): boolean {
  const selected = selectedConvertibleItems(state);
  if (selected.size === 0) return false;

  const targetListType = state.schema.nodes[targetListName];
  const targetItemType = state.schema.nodes[
    targetListName === "taskList" ? "taskItem" : "listItem"
  ];
  if (!targetListType || !targetItemType) return false;

  const transformedContent = transformChildren(
    state.doc,
    selected,
    targetListName,
    targetListType,
    targetItemType,
  );
  const transformedDoc = state.doc.type.create(
    state.doc.attrs,
    Fragment.fromArray(transformedContent),
    state.doc.marks,
  );
  if (!dispatch) return true;

  const diffStart = state.doc.content.findDiffStart(
    transformedDoc.content,
  );
  const diffEnd = state.doc.content.findDiffEnd(
    transformedDoc.content,
  );
  if (diffStart === null && diffEnd === null) return true;
  if (diffStart === null || diffEnd === null) return false;

  const { selection } = state;
  const transaction = state.tr.replace(
    diffStart,
    diffEnd.a,
    transformedDoc.slice(diffStart, diffEnd.b),
  );
  if (selection instanceof TextSelection) {
    const anchor = mappedTextPosition(
      transformedDoc,
      selection.$anchor.parent,
      selection.$anchor.parentOffset,
    );
    const head = mappedTextPosition(
      transformedDoc,
      selection.$head.parent,
      selection.$head.parentOffset,
    );
    if (anchor !== undefined && head !== undefined) {
      transaction.setSelection(
        TextSelection.create(transaction.doc, anchor, head),
      );
    }
  } else if (selection instanceof AllSelection) {
    transaction.setSelection(new AllSelection(transaction.doc));
  }
  if (state.storedMarks) transaction.setStoredMarks(state.storedMarks);
  dispatch(transaction.scrollIntoView());
  return true;
}

function sinkListItemAcrossAdjacentLists(
  editor: Editor,
  itemType: "listItem" | "taskItem",
): boolean {
  if (editor.can().sinkListItem(itemType)) {
    return editor.commands.sinkListItem(itemType);
  }

  const { $from } = editor.state.selection;
  let itemDepth = $from.depth;
  while (itemDepth > 0 && $from.node(itemDepth).type.name !== itemType) {
    itemDepth -= 1;
  }
  if (itemDepth < 2) return false;

  const listDepth = itemDepth - 1;
  const parentDepth = listDepth - 1;
  const list = $from.node(listDepth);
  const listIndex = $from.index(parentDepth);
  if (listIndex === 0) return false;

  const previousList = $from.node(parentDepth).child(listIndex - 1);
  const joinPosition = $from.before(listDepth);
  if (previousList.type === list.type) {
    if (!canJoin(editor.state.doc, joinPosition)) return false;

    return editor
      .chain()
      .command(({ tr }) => {
        tr.join(joinPosition);
        return true;
      })
      .sinkListItem(itemType)
      .run();
  }

  const { selection } = editor.state;
  if (
    itemType !== "listItem" ||
    list.type.name !== "bulletList" ||
    previousList.type.name !== "taskList" ||
    selection.$to.depth < listDepth ||
    selection.$to.node(listDepth) !== list ||
    $from.index(listDepth) !== 0
  ) {
    return false;
  }

  const lastSelectedItem = selection.$to.index(listDepth);
  let movedSize = 0;
  for (let index = 0; index <= lastSelectedItem; index += 1) {
    movedSize += list.child(index).nodeSize;
  }

  const movedItems = list.content.cut(0, movedSize);
  const remainingItems = list.content.cut(movedSize);
  const previousTask = previousList.lastChild;
  if (!previousTask) return false;

  const existingNestedList =
    previousTask.lastChild?.type === list.type
      ? previousTask.lastChild
      : undefined;
  const nestedList = existingNestedList
    ? existingNestedList.copy(
        existingNestedList.content.append(movedItems),
      )
    : list.type.create(list.attrs, movedItems);
  const nestedContent = existingNestedList
    ? previousTask.content.replaceChild(
        previousTask.childCount - 1,
        nestedList,
      )
    : previousTask.content.append(Fragment.from(nestedList));
  if (!previousTask.type.validContent(nestedContent)) return false;

  const updatedTask = previousTask.copy(nestedContent);
  const updatedPreviousList = previousList.copy(
    previousList.content.replaceChild(
      previousList.childCount - 1,
      updatedTask,
    ),
  );
  const replacement =
    remainingItems.size > 0
      ? Fragment.fromArray([
          updatedPreviousList,
          list.copy(remainingItems),
        ])
      : Fragment.from(updatedPreviousList);

  const previousListStart = joinPosition - previousList.nodeSize;
  const previousTaskStart =
    previousListStart +
    1 +
    previousList.content.size -
    previousTask.nodeSize;
  const nestedListStart = existingNestedList
    ? previousTaskStart +
      1 +
      previousTask.content.size -
      existingNestedList.nodeSize
    : previousTaskStart + 1 + previousTask.content.size;
  const movedContentStart =
    nestedListStart +
    1 +
    (existingNestedList?.content.size ?? 0);
  const currentContentStart = joinPosition + 1;
  const selectionFrom =
    movedContentStart + selection.from - currentContentStart;
  const selectionTo =
    movedContentStart + selection.to - currentContentStart;

  return editor.commands.command(({ tr }) => {
    tr.replaceWith(
      previousListStart,
      joinPosition + list.nodeSize,
      replacement,
    );
    tr.setSelection(
      TextSelection.create(tr.doc, selectionFrom, selectionTo),
    );
    tr.scrollIntoView();
    return true;
  });
}

function liftBulletListItemOutOfTask(
  editor: Editor,
): boolean | undefined {
  const { selection } = editor.state;
  const { $from, $to } = selection;
  let itemDepth = $from.depth;
  while (
    itemDepth > 0 &&
    $from.node(itemDepth).type.name !== "listItem"
  ) {
    itemDepth -= 1;
  }
  if (itemDepth < 4) return undefined;

  const listDepth = itemDepth - 1;
  const taskDepth = listDepth - 1;
  const taskListDepth = taskDepth - 1;
  const list = $from.node(listDepth);
  const task = $from.node(taskDepth);
  const taskList = $from.node(taskListDepth);
  if (
    list.type.name !== "bulletList" ||
    task.type.name !== "taskItem" ||
    taskList.type.name !== "taskList"
  ) {
    return undefined;
  }

  if (
    $to.depth < listDepth ||
    $to.node(listDepth) !== list ||
    $from.index(taskDepth) !== task.childCount - 1 ||
    $from.index(taskListDepth) !== taskList.childCount - 1
  ) {
    return false;
  }

  const firstSelectedItem = $from.index(listDepth);
  const lastSelectedItem = $to.index(listDepth);
  if (lastSelectedItem !== list.childCount - 1) return false;

  let keptSize = 0;
  for (let index = 0; index < firstSelectedItem; index += 1) {
    keptSize += list.child(index).nodeSize;
  }

  const keptItems = list.content.cut(0, keptSize);
  const movedItems = list.content.cut(keptSize);
  const nestedContent =
    keptItems.size > 0
      ? task.content.replaceChild(
          task.childCount - 1,
          list.copy(keptItems),
        )
      : task.content.cut(0, task.content.size - list.nodeSize);
  if (!task.type.validContent(nestedContent)) return false;

  const updatedTask = task.copy(nestedContent);
  const updatedTaskList = taskList.copy(
    taskList.content.replaceChild(
      taskList.childCount - 1,
      updatedTask,
    ),
  );
  const containerDepth = taskListDepth - 1;
  const taskListIndex = $from.index(containerDepth);
  const container = $from.node(containerDepth);
  const nextList =
    taskListIndex + 1 < container.childCount &&
    container.child(taskListIndex + 1).type === list.type
      ? container.child(taskListIndex + 1)
      : undefined;
  const outdentedList = list.copy(
    nextList ? movedItems.append(nextList.content) : movedItems,
  );
  const replacement = Fragment.fromArray([
    updatedTaskList,
    outdentedList,
  ]);

  const taskListStart = $from.before(taskListDepth);
  const taskListEnd = $from.after(taskListDepth);
  const nestedListStart =
    $from.before(taskDepth) +
    1 +
    task.content.size -
    list.nodeSize;
  const oldMovedContentStart = nestedListStart + 1 + keptSize;
  const newMovedContentStart =
    taskListStart + updatedTaskList.nodeSize + 1;
  const selectionFrom =
    newMovedContentStart + selection.from - oldMovedContentStart;
  const selectionTo =
    newMovedContentStart + selection.to - oldMovedContentStart;

  return editor.commands.command(({ tr }) => {
    tr.replaceWith(
      taskListStart,
      taskListEnd + (nextList?.nodeSize ?? 0),
      replacement,
    );
    tr.setSelection(
      TextSelection.create(tr.doc, selectionFrom, selectionTo),
    );
    tr.scrollIntoView();
    return true;
  });
}

const StickyShortcuts = Extension.create({
  name: "stickyShortcuts",
  priority: 1_000,
  addKeyboardShortcuts() {
    return {
      Backspace: () => {
        if (this.editor.commands.undoInputRule()) return true;
        if (liftEmptyListItemOnly(this.editor)) return true;
        return this.editor.commands.command(({ state, dispatch }) =>
          deleteEmptyParagraphAfterList(state, dispatch),
        );
      },
      Enter: () => liftEmptyListItemOnly(this.editor),
      Tab: () => {
        const itemType = activeListItemType(this.editor);
        if (itemType) {
          sinkListItemAcrossAdjacentLists(this.editor, itemType);
        }
        // Keep WebKit from moving keyboard focus into a task checkbox.
        return true;
      },
      "Shift-Tab": () => {
        const itemType = activeListItemType(this.editor);
        if (itemType === "listItem") {
          const liftedFromTask = liftBulletListItemOutOfTask(
            this.editor,
          );
          if (liftedFromTask !== undefined) return true;
          if (this.editor.commands.liftListItem("listItem")) {
            return true;
          }
        } else if (
          itemType === "taskItem" &&
          this.editor.commands.liftListItem("taskItem")
        ) {
          return true;
        }

        const { state, view } = this.editor;
        const { $from } = state.selection;
        if (
          state.selection.empty &&
          $from.depth === 1 &&
          $from.parent.isTextblock
        ) {
          const previousLine = Selection.findFrom(
            state.doc.resolve($from.before()),
            -1,
            true,
          );
          if (previousLine) {
            const currentContent = state.doc.slice(
              $from.start(),
              $from.end(),
            ).content;
            const previousEnd = previousLine.$from.end();
            const needsSpace =
              previousLine.$from.parent.textContent.length > 0 &&
              $from.parent.textContent.length > 0 &&
              !/\s$/.test(previousLine.$from.parent.textContent) &&
              !/^\s/.test($from.parent.textContent);
            const transaction = state.tr.delete(
              $from.before(),
              $from.after(),
            );
            let insertionEnd = previousEnd;
            if (needsSpace) {
              transaction.insertText(" ", insertionEnd);
              insertionEnd += 1;
            }
            transaction.insert(insertionEnd, currentContent);
            insertionEnd += currentContent.size;
            view.dispatch(
              transaction
                .setSelection(
                  TextSelection.create(transaction.doc, insertionEnd),
                )
                .scrollIntoView(),
            );
          }
        }

        // Always consume the shortcut so WebKit cannot focus titlebar controls.
        return true;
      },
      "Mod-Shift-0": () => {
        if (
          this.editor.commands.command(({ state, dispatch }) =>
            transformSelectedListItems(state, dispatch, "bulletList"),
          )
        ) {
          return true;
        }
        return this.editor.commands.toggleBulletList();
      },
      "Mod-Shift-9": () => {
        if (
          this.editor.commands.command(({ state, dispatch }) =>
            transformSelectedListItems(state, dispatch, "taskList"),
          )
        ) {
          return true;
        }
        return this.editor.commands.toggleTaskList();
      },
      "Mod-Shift-x": () => {
        const result = removeCheckedTasks(this.editor.getJSON().content ?? []);
        if (!result.removed) return true;

        return this.editor.commands.setContent({
          type: "doc",
          content:
            result.nodes.length > 0
              ? result.nodes
              : [{ type: "paragraph" }],
        });
      },
      "Mod-Shift-c": () => {
        if (!this.editor.isActive("taskItem")) return false;
        const checked = Boolean(
          this.editor.getAttributes("taskItem").checked,
        );
        return this.editor.commands.updateAttributes("taskItem", {
          checked: !checked,
        });
      },
      "Mod-Shift-s": () => {
        const { selection } = this.editor.state;
        if (!selection.empty) return this.editor.commands.toggleStrike();

        const cursor = selection.from;
        const blockStart = selection.$from.start();
        const blockEnd = selection.$from.end();
        if (blockStart === blockEnd) {
          return this.editor.commands.toggleStrike();
        }

        const strike = this.editor.schema.marks.strike;
        let hasStrike = false;
        this.editor.state.doc.nodesBetween(blockStart, blockEnd, (node) => {
          if (node.isText && strike && strike.isInSet(node.marks)) {
            hasStrike = true;
          }
        });

        const chain = this.editor
          .chain()
          .setTextSelection({ from: blockStart, to: blockEnd });
        if (hasStrike) chain.unsetStrike();
        else chain.setStrike();
        return chain.setTextSelection(cursor).run();
      },
    };
  },
});

export function createEditorExtensions() {
  return [
    StarterKit.configure({
      heading: {
        levels: [1, 2, 3],
      },
    }),
    TaskList,
    TaskItem.configure({
      nested: true,
    }),
    Markdown,
    MarkdownPaste,
    NoteSearch,
    TaskSelectionCheck,
    StickyShortcuts,
    Placeholder.configure({
      placeholder: "Empty Note",
    }),
  ];
}
