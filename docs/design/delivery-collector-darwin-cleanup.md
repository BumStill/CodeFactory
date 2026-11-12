# 收集器 Mac 清理错误修复

## 证据与根因边界
Mac 独立验收基于 b412497fe0510f3a938e85101d4efbffdee8c9d6，Homebrew Python3.14.6，8过2错误无skip。非零exporter与正常编译产物在waitid WNOWAIT后killpg出现EPERM，finally跳过close/reap，并出现ResourceWarning。该实际序列不兼容已证实；仅凭回执不能证明Darwin所有EPERM都代表zombie-only组。

## 修复
不忽略EPERM、不改安全设置。改用受控Python supervisor保持组长存活：启动exporter并等待其退出，以独立64字节上限状态管道报告结果；关闭自身stdout，阻塞控制stdin，直到宿主清理。exporter不继承状态fd。宿主在同一deadline下复用selector收集输出与状态。组长不先reap，不对已回收PID发组信号。

所有kill异常均记录为CLEANUP_UNCONFIRMED；即使killpg异常仍尝试终止未回收的直接组长，独立关闭管道、限时wait回收。原始失败保留并附cleanup note；原本成功但清理失败则拒绝返回。真正组清理失败时不声称全部后代退出；setsid逃逸仍不在证明范围。

## 红绿与验收
新3项测试先红：13项中3 errors，collector-darwin-red.log。最终15/15 Linux通过，ResourceWarning作为错误运行；涵盖Darwin式已退出组EPERM注入、真实权限失败、非OSError异常、活跃exporter超时与回收、缺失/畸形/超长状态、启动失败，保留全部原正常Python/Node/C与延迟子进程正例。独立QA复核。异常组kill下有限exporter自然退出不等于成功清理，测试明确区分。

命令：python3 -W error::ResourceWarning -m unittest discover -s tests -p test_delivery_candidate_collector.py -v

Linux注入不是Mac原生复验；请在Mac精确候选运行同一命令。Windows、GUI/provider未运行。Rust正常交付/hooks不变。始终未准入，未实现磁盘封存或生产handler接线。

## 治理
仍无精确候选canonical通过：旧14文件映射问题和本地pnpm依赖初始化ENOENT保留；不重复已知失败安装、不改registry/validator/hooks/trust root/保护规则。仅本地增量包移交，无push/PR/合并/发布。
