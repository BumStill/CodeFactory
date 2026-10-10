# 交付锁与网络恢复

### Requirements Traceability

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| CF-LEASE-R1 | 交付锁的生命周期有边界：`deliver_changes` 返回后（无论成功、失败还是停在等待），或者交付停下来等外部条件时，锁立即释放。同一个 objective 再次发起交付时，直接接上自己的那次交付，绝不等待自己持有的锁 | 集成测试：交付返回后立刻再次交付，不被自己挡住；停在等待时锁已释放 |
| CF-LEASE-R2 | 持有者已经不在（进程重启、那次交付已结束或被放弃）的锁，在有界时间内被回收；回收时留下记录 | 测试：模拟残留锁，有界时间内被回收并有审计记录 |
| CF-LEASE-R3 | 交付里的网络步骤（fetch / push / 开 PR / 查 PR）遇到瞬时网络错误（SSL、连接重置、超时、5xx），先做有界退避重试，不是失败一次就停；重试用尽后，用大白话说明卡在哪一步、已保留什么（提交、分支、PR） | 测试：注入瞬时错误，重试后成功；持续失败时给出如实说明 |
| CF-LEASE-R4 | 交付进程的网络环境与用户终端一致：git 与 gh 能用到用户已配置的代理（例如 ~/.gitconfig 的 http.proxy、HTTPS_PROXY），不因为隔离配置而直连失败；隔离只限制写操作的范围，不切断网络配置 | 测试：用合成配置断言交付子进程能读到代理设置 |
| CF-LEASE-R5 | 交付卡住超过有界时间时，会话要明说「卡在哪一步、在等什么、谁能解开」，不能沉默停住 | 测试：卡住超时后的文案 |

### Applicable Harnesses
Spec Harness；Compatibility Harness（已有的 delivery_runs 与锁记录）；Observation Harness（锁的获取、释放、回收可追溯）；Release Harness（交付路径）；AI Collaboration Harness。

### 测试矩阵
- 交付成功、失败、停在等待，三种结果之后立即再次交付。
- app 重启后残留的锁。
- fetch / push / 开 PR 各自遇到瞬时错误与持续错误。
- 配置了代理的环境。
