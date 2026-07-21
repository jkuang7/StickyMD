import { TaskItem, TaskList } from "@tiptap/extension-list";
import { Placeholder } from "@tiptap/extension-placeholder";
import { Extension, type JSONContent } from "@tiptap/core";
import { Selection, TextSelection } from "@tiptap/pm/state";
import { StarterKit } from "@tiptap/starter-kit";

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

const StickyShortcuts = Extension.create({
  name: "stickyShortcuts",
  priority: 1_000,
  addKeyboardShortcuts() {
    return {
      Tab: () => {
        if (this.editor.isActive("taskItem")) {
          this.editor.commands.sinkListItem("taskItem");
          return true;
        }
        if (this.editor.isActive("listItem")) {
          this.editor.commands.sinkListItem("listItem");
          return true;
        }
        // Keep WebKit from moving keyboard focus into a task checkbox.
        return true;
      },
      "Shift-Tab": () => {
        if (
          this.editor.isActive("taskItem") &&
          this.editor.commands.liftListItem("taskItem")
        ) {
          return true;
        }
        if (
          this.editor.isActive("listItem") &&
          this.editor.commands.liftListItem("listItem")
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
      "Mod-Shift-0": () => this.editor.commands.toggleBulletList(),
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
    StickyShortcuts,
    Placeholder.configure({
      placeholder: "Empty Note",
    }),
  ];
}
