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

export function Button({ variant = "primary", type = "button", className = "", ...props }: ButtonProps) {
  return (
    <button
      {...props}
      type={type}
      className={`trl-button trl-button--${variant} ${className}`.trim()}
    />
  );
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
  return (
    <dialog
      {...props}
      aria-labelledby={titleId}
      className={`trl-dialog ${className}`.trim()}
    >
      <header className="trl-dialog__header">
        <h2 id={titleId}>{title}</h2>
        <form method="dialog">
          <Button aria-label={onCloseLabel} variant="secondary">
            ×
          </Button>
        </form>
      </header>
      <div className="trl-dialog__body">{children}</div>
    </dialog>
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
  return (
    <div className="trl-table-scroll" tabIndex={0} aria-label={`${caption} table region`}>
      <table className="trl-table">
        <caption>{caption}</caption>
        <thead>
          <tr>
            {columns.map((column) => (
              <th {...column.headerProps} key={column.key} scope="col">
                {column.header}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => (
            <tr key={rowKey(row)}>
              {columns.map((column) => (
                <td key={column.key}>{column.render(row)}</td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function StatusBadge({ children }: PropsWithChildren) {
  return <span className="trl-status-badge">{children}</span>;
}
