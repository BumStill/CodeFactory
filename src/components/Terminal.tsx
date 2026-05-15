// SPDX-License-Identifier: Apache-2.0
import { useEffect, useRef } from "react";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import "@xterm/xterm/css/xterm.css";

interface TerminalProps {
  id: string;
}

export default function Terminal({ id }: TerminalProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  // Keep stable refs so the cleanup closure always sees the latest instances.
  const termRef = useRef<XTerm | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const unlistenRef = useRef<UnlistenFn | null>(null);

  useEffect(() => {
    if (!containerRef.current) return;

    // ----- Create terminal -----
    const term = new XTerm({
      cursorBlink: true,
      fontSize: 13,
      fontFamily: '"Cascadia Code", "Fira Code", Menlo, monospace',
      theme: {
        background: "#1e1e1e",
        foreground: "#d4d4d4",
        cursor: "#d4d4d4",
        selectionBackground: "#264f78",
      },
    });

    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(containerRef.current);
    fitAddon.fit();

    termRef.current = term;
    fitRef.current = fitAddon;

    // ----- Invoke backend: create pty -----
    invoke("terminal_create", {
      id,
      cols: term.cols,
      rows: term.rows,
    }).catch((err) => {
      term.write(`\r\n\x1b[31mFailed to start terminal: ${err}\x1b[0m\r\n`);
    });

    // ----- Listen for output from the pty -----
    listen<string>(`terminal-output:${id}`, (event) => {
      term.write(event.payload);
    }).then((unlisten) => {
      unlistenRef.current = unlisten;
    });

    // ----- Send keystrokes to the pty -----
    const dataDispose = term.onData((data) => {
      invoke("terminal_write", { id, data }).catch(() => {
        // Swallow; pty may have exited.
      });
    });

    // ----- Resize observer -----
    const observer = new ResizeObserver(() => {
      if (!fitRef.current || !termRef.current) return;
      try {
        fitRef.current.fit();
        invoke("terminal_resize", {
          id,
          cols: termRef.current.cols,
          rows: termRef.current.rows,
        }).catch(() => {});
      } catch {
        // FitAddon throws if the element has zero dimensions.
      }
    });
    if (containerRef.current) {
      observer.observe(containerRef.current);
    }

    // ----- Cleanup -----
    return () => {
      dataDispose.dispose();
      observer.disconnect();
      unlistenRef.current?.();
      invoke("terminal_kill", { id }).catch(() => {});
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
    };
  }, [id]);

  return (
    <div
      ref={containerRef}
      style={{ width: "100%", height: "100%", overflow: "hidden" }}
    />
  );
}
