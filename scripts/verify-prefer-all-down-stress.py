#!/usr/bin/env python3
"""在 CPU 压力下验证 prefer-all-down fixture；不重试失败、不改变测试超时。"""
import argparse
import multiprocessing
import os
import subprocess
import tempfile
from pathlib import Path


def load_cpu(stop):
    value = 1
    while not stop.is_set():
        for _ in range(10000):
            value = (value * 1664525 + 1013904223) & 0xFFFFFFFF


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path, help="cargo test 生成的 codefactory_lib 测试可执行文件")
    parser.add_argument("--iterations", type=int, default=100)
    parser.add_argument("--suite-iterations", type=int, default=20)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    assert args.iterations > 0 and args.suite_iterations > 0
    stop = multiprocessing.Event()
    workers = [multiprocessing.Process(target=load_cpu, args=(stop,))
               for _ in range(os.cpu_count() or 1)]
    try:
        for worker in workers:
            worker.start()
        print(f"CPU workers: {len(workers)}; PIDs: {[w.pid for w in workers]}", flush=True)
        with tempfile.TemporaryDirectory(prefix="prefer-all-down-stress-") as directory:
            print(f"Log directory: {directory}", flush=True)
            # Running outside the source checkout also proves the actual test
            # artifact exercises TCP + SQLite without relying on the source cwd.
            for label, count, filters in [
                ("target", args.iterations,
                 ["agent::model_transport::tests::prefer_mode_waits_only_after_every_endpoint_is_unavailable", "--exact"]),
                ("suite", args.suite_iterations, ["agent::model_transport::tests::"]),
            ]:
                for iteration in range(1, count + 1):
                    command = [str(binary), *filters, "--test-threads=8"]
                    result = subprocess.run(command, cwd=directory, text=True,
                                            encoding="utf-8", errors="replace",
                                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                            timeout=60, check=False)
                    Path(directory, f"{label}-{iteration}.log").write_text(result.stdout)
                    if result.returncode != 0:
                        raise RuntimeError(f"{label} iteration {iteration} failed:\n{result.stdout}")
                    expected = "1 passed; 0 failed" if label == "target" else "45 passed; 0 failed"
                    assert expected in result.stdout, result.stdout
                    if iteration % 10 == 0:
                        print(f"{label}: {iteration}/{count} PASS", flush=True)
                print(f"{label}: ALL {count} PASS", flush=True)
    finally:
        stop.set()
        for worker in workers:
            worker.join(timeout=5)
            if worker.is_alive():
                worker.terminate()
                worker.join(timeout=5)
            assert not worker.is_alive(), f"CPU worker {worker.pid} did not stop"


if __name__ == "__main__":
    main()
