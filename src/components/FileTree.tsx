// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "react";
import { invoke } from "../lib/tauri";

interface FileNode {
  name: string;
  path: string;
  is_dir: boolean;
  children?: FileNode[];
}

function fileIcon(node: FileNode): string {
  if (node.is_dir) return "📁";
  const ext = node.name.split(".").pop()?.toLowerCase() ?? "";
  if (ext === "rs") return "🦀";
  if (ext === "ts" || ext === "tsx") return "💙";
  if (ext === "json") return "⚙";
  return "📄";
}

interface FileNodeItemProps {
  node: FileNode;
  root: string;
  onSelectFile: (path: string) => void;
  depth: number;
}

function FileNodeItem({ node, root, onSelectFile, depth }: FileNodeItemProps) {
  const [open, setOpen] = useState(false);
  const [children, setChildren] = useState<FileNode[]>(node.children ?? []);
  const [loaded, setLoaded] = useState(node.children !== undefined);

  const handleClick = async () => {
    if (!node.is_dir) {
      onSelectFile(node.path);
      return;
    }
    if (!open && !loaded) {
      try {
        const nodes = await invoke<FileNode[]>("list_dir", {
          path: node.path,
          root,
          depth: 1,
        });
        setChildren(nodes);
        setLoaded(true);
      } catch {
        // ignore errors silently
      }
    }
    setOpen((o) => !o);
  };

  return (
    <li>
      <button
        onClick={handleClick}
        className="w-full flex items-center gap-1 px-2 py-0.5 text-left text-xs text-gray-400 hover:text-gray-200 hover:bg-surface-2 transition-colors rounded truncate"
        style={{ paddingLeft: `${8 + depth * 10}px` }}
        title={node.path}
      >
        <span className="shrink-0">{fileIcon(node)}</span>
        <span className="truncate">{node.name}</span>
      </button>
      {node.is_dir && open && children.length > 0 && (
        <ul>
          {children.map((child) => (
            <FileNodeItem
              key={child.path}
              node={child}
              root={root}
              onSelectFile={onSelectFile}
              depth={depth + 1}
            />
          ))}
        </ul>
      )}
    </li>
  );
}

interface FileTreeProps {
  cwd: string;
  onSelectFile: (path: string) => void;
}

export function FileTree({ cwd, onSelectFile }: FileTreeProps) {
  const [nodes, setNodes] = useState<FileNode[]>([]);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (!cwd) return;
    setNodes([]);
    invoke<FileNode[]>("list_dir", { path: cwd, root: cwd, depth: 2 })
      .then(setNodes)
      .catch(() => {});
  }, [cwd]);

  if (!cwd) return null;

  return (
    <div className="border-t border-border">
      <button
        onClick={() => setOpen((o) => !o)}
        className="w-full flex items-center gap-1 px-3 py-2 text-xs font-semibold text-gray-400 uppercase tracking-wider hover:text-gray-300 transition-colors"
      >
        <span className="flex-1 text-left">Files</span>
        <span className="text-gray-600">{open ? "▾" : "▸"}</span>
      </button>
      {open && (
        <ul className="pb-2 overflow-y-auto max-h-60">
          {nodes.map((node) => (
            <FileNodeItem
              key={node.path}
              node={node}
              root={cwd}
              onSelectFile={onSelectFile}
              depth={0}
            />
          ))}
          {nodes.length === 0 && (
            <li className="px-3 py-1 text-xs text-gray-700">Empty</li>
          )}
        </ul>
      )}
    </div>
  );
}
