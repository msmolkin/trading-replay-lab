import { createElement } from "react";
import type {
  ButtonHTMLAttributes,
  DialogHTMLAttributes,
  PropsWithChildren,
  ReactNode,
  ThHTMLAttributes,
} from "react";

export type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "primary" | "secondary" | "danger";
};

export function Button({
  variant = "primary",
  type = "button",
  className = "",
  ...props
}: ButtonProps) {
  return createElement("button", {
    ...props,
    type,
    className: `trl-button trl-button--${variant} ${className}`.trim(),
  });
}

export type DialogProps = DialogHTMLAttributes<HTMLDialogElement> & {
  title: string;
  titleId: string;
  onCloseLabel?: string;
};

export function Dialog({
  title,
  titleId,
  onCloseLabel = "Close",
  children,
  className = "",
  ...props
}: PropsWithChildren<DialogProps>) {
  return createElement(
    "dialog",
    {
      ...props,
      "aria-labelledby": titleId,
      className: `trl-dialog ${className}`.trim(),
    },
    createElement(
      "header",
      { className: "trl-dialog__header" },
      createElement("h2", { id: titleId }, title),
      createElement(
        "form",
        { method: "dialog" },
        createElement(Button, { "aria-label": onCloseLabel, variant: "secondary" }, "×"),
      ),
    ),
    createElement("div", { className: "trl-dialog__body" }, children),
  );
}

export type TableColumn<Row> = {
  key: string;
  header: ReactNode;
  render: (row: Row) => ReactNode;
  headerProps?: ThHTMLAttributes<HTMLTableCellElement>;
};

export type DataTableProps<Row> = {
  caption: string;
  columns: readonly TableColumn<Row>[];
  rows: readonly Row[];
  rowKey: (row: Row) => string;
};

export function DataTable<Row>({ caption, columns, rows, rowKey }: DataTableProps<Row>) {
  return createElement(
    "div",
    { className: "trl-table-scroll", tabIndex: 0, "aria-label": `${caption} table region` },
    createElement(
      "table",
      { className: "trl-table" },
      createElement("caption", null, caption),
      createElement(
        "thead",
        null,
        createElement(
          "tr",
          null,
          ...columns.map((column) =>
            createElement(
              "th",
              { ...column.headerProps, key: column.key, scope: "col" },
              column.header,
            ),
          ),
        ),
      ),
      createElement(
        "tbody",
        null,
        ...rows.map((row) =>
          createElement(
            "tr",
            { key: rowKey(row) },
            ...columns.map((column) =>
              createElement("td", { key: column.key }, column.render(row)),
            ),
          ),
        ),
      ),
    ),
  );
}

export function StatusBadge({ children }: PropsWithChildren) {
  return createElement("span", { className: "trl-status-badge" }, children);
}
