from pathlib import Path
import unittest


REPO_ROOT = Path(__file__).resolve().parents[1]
ADAPTER = REPO_ROOT / "codefactory_bench" / "agent.py"
RUST_AGENT = REPO_ROOT / "src-tauri" / "src" / "agent" / "mod.rs"
RUST_CORE = REPO_ROOT / "src-tauri" / "crates" / "agent-core" / "src" / "lib.rs"
RUST_HEADLESS = REPO_ROOT / "src-tauri" / "crates" / "agent-headless" / "src" / "main.rs"
SHARED_CONTRACT = REPO_ROOT / "agent_contracts" / "execution_completion.md"
EVALUATION_SPEC = (
    REPO_ROOT / "docs" / "specs" / "feature-specs" / "terminal-bench-21-evaluation.md"
)


class BenchmarkIntegrityTests(unittest.TestCase):
    def test_adapter_has_no_fixed_task_fingerprints_or_repairs(self) -> None:
        source = ADAPTER.read_text(encoding="utf-8").lower()
        forbidden = [
            "_verification_hint_from_instruction",
            "implementation hint: this is",
            "secret_sanitization_task",
            "sqlite_query_optimization_task",
            "grpc_kv_store_task",
            "html_javascript_filter_task",
            "extract_elf_task",
            "torch_tensor_parallelism_task",
            "install_windows_task",
            "qemu_startup_task",
            "circuit_fibsqrt_task",
            "sparql_university_task",
            "nginx_request_logging_task",
            "git_webserver_task",
            "pyknotid_cython_build_task",
            "protein_assembly_gblock_task",
            "hf_dataset_token_count_task",
            "_pyknotid_cython_build_auto_repair_command",
            "_git_webserver_auto_repair_command",
            "_install_windows_auto_repair_command",
            "_circuit_fibsqrt_auto_repair_command",
        ]

        offenders = [fragment for fragment in forbidden if fragment in source]
        self.assertEqual([], offenders, f"task-specific adapter logic found: {offenders}")

    def test_adapter_never_reads_hidden_verifier_files(self) -> None:
        source = ADAPTER.read_text(encoding="utf-8").lower()
        self.assertNotIn("/tests/test.sh", source)
        self.assertNotIn("/tests/", source)

    def test_product_runtime_has_no_fixed_terminal_bench_task_branches(self) -> None:
        source = "\n".join(
            path.read_text(encoding="utf-8").lower()
            for path in (RUST_AGENT, RUST_CORE, RUST_HEADLESS)
        )
        forbidden = [
            "extract-elf",
            "torch-tensor-parallelism",
            "caffe-cifar-10",
            "query-optimize",
            "qemu-startup",
            "write-compressor",
            "count-dataset-tokens",
            "configure-git-webserver",
        ]

        offenders = [fragment for fragment in forbidden if fragment in source]
        self.assertEqual([], offenders, f"fixed benchmark task found in product runtime: {offenders}")

    def test_product_and_headless_load_the_same_completion_contract(self) -> None:
        self.assertTrue(SHARED_CONTRACT.is_file(), "shared execution contract is missing")
        rust_source = RUST_AGENT.read_text(encoding="utf-8")
        adapter_source = ADAPTER.read_text(encoding="utf-8")
        self.assertIn(
            'include_str!("../../../agent_contracts/execution_completion.md")',
            rust_source,
        )
        self.assertIn("_load_execution_contract", adapter_source)
        self.assertIn("execution_contract_sha256", adapter_source)

    def test_python_adapter_is_only_a_sidecar_bridge(self) -> None:
        source = ADAPTER.read_text(encoding="utf-8")
        self.assertNotIn("chat/completions", source)
        self.assertNotIn("_chat_completion", source)
        self.assertNotIn("verification_hint", source)
        self.assertIn("create_subprocess_exec", source)
        self.assertIn('"tool_request"', source)
        self.assertIn('"tool_result"', source)

    def test_active_evaluation_spec_forbids_task_specific_repairs(self) -> None:
        source = EVALUATION_SPEC.read_text(encoding="utf-8")
        forbidden = [
            "Binary artifact task-family repair",
            "Distributed autograd task-family repair",
            "ELF memory extraction 任务会提示",
            "PyTorch tensor-parallel linear 任务会提示",
            "对于 ELF memory extraction",
            "PT_LOAD",
            "readUInt32LE",
            "ColumnParallelLinear",
            "torch.autograd.Function",
        ]

        offenders = [fragment for fragment in forbidden if fragment in source]
        self.assertEqual([], offenders, f"obsolete task-specific spec found: {offenders}")
        self.assertIn("固定 18 题最终门禁", source)
        self.assertIn("18 / 18", source)
        self.assertIn("clean Linux/x86", source)
        self.assertIn("继承 Harbor 已生效的网络策略", source)
        self.assertNotIn("Benchmark sandbox 默认不得允许外网工具调用", source)


if __name__ == "__main__":
    unittest.main()
