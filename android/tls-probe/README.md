# POKE4S TLS probe（一次性测试装置）

用途：在决定 PR-B 要不要上 HTTPS、用什么信任模型之前，先用真机回答四个问题。

1. POKE4S 能不能握手 TLS 1.2 / 1.3？
2. App 级嵌入一个自签 CA，能不能连上本地 HTTPS？
3. 证书错了会不会被拒（不是 trust-all）？
4. 证书固定（SPKI pinning）在设备上可行吗？

**这不是产品代码**，不参与 CI，不进 APK 发布物。测完可以整个删掉。

## 步骤

```bash
# 1. 生成一次性证书（两套 CA：可信 / 不可信）
bash gen-certs.sh
#    把输出的 SPKI pin 填进 MainActivity.EXPECTED_PIN

# 2. 构建并安装
export JAVA_HOME="$HOME/android-sdk/jdk17" ANDROID_SDK_ROOT="$HOME/android-sdk"
echo "sdk.dir=$ANDROID_SDK_ROOT" > local.properties
./gradlew assembleDebug
adb install -r app/build/outputs/apk/debug/app-debug.apk

# 3. 两个本地 HTTPS 服务（不需要开 Mac 的局域网监听）
python3 server.py --port 8767                        # 可信 CA
python3 server.py --port 8768 --cert-dir certs/ca2   # 不可信 CA
adb reverse tcp:8767 tcp:8767
adb reverse tcp:8768 tcp:8768

# 4. 跑，读 logcat
adb shell am start -n com.aieink.tlsprobe/.MainActivity
adb logcat -s AICCTlsProbe
adb pull /sdcard/Android/data/com.aieink.tlsprobe/files/probe.txt
```

`server.py` 需要 Python 3.10+（TLS 1.3）。系统自带的 `/usr/bin/python3` 是 3.9 + LibreSSL，
跑不了 1.3 —— 这本身就是下面第 2 条结论的一部分。

### Boox 设备的一个坑

新装应用在 POKE4S 上是 `DISABLED_USER` 状态，`am start` 会报 "Activity class does not exist"。先执行：

```bash
adb shell pm enable com.aieink.tlsprobe
```

## 实测结果（2026-11-16）

设备：Poke4S / Onyx / Android 11（API 30）/ `2025-04-02_08-15_v4.0-rel` / arm64-v8a / 758×1024。

```
CASE tls12_handshake              => PASS: TLSv1.2
CASE tls13_handshake              => PASS: TLSv1.3
CASE https_health_200             => PASS: status 200
CASE https_status_401_without_token => PASS: status 401
CASE untrusted_server_rejected    => PASS: rejected: SSLHandshakeException
CASE pinning_match                => PASS: Si8yl4K5Gvxq14fACg95+3MJ2QMTCXRC3dM5DJ9sI1Y=
CASE pinning_mismatch_detected    => PASS: wrong pin does not match
CASE cleartext_blocked            => PASS: blocked: IOException
SUMMARY pass=8 fail=0 elapsedMs=512
```

全程 512 ms（8 个用例），TLS 在这个设备上没有可感知的额外成本。

## 结论与对 PR-B 的影响

1. **设备侧没问题**：TLS 1.2/1.3、自签 CA 的 App 级信任、SPKI pinning、明文拦截，四项都成立。
   所以 PR-B 不需要为兼容性留 HTTP 后门。
2. **但桥接服务端目前只能到 TLS 1.2**：App 用 `/usr/bin/python3`（3.9.6 + LibreSSL 2.8.3，
   `HAS_TLSv1_3 == False`）启动 `server.py`。要上 1.3，必须换解释器（bundled python 或要求
   用户装 3.10+），这会改变打包与安装体积，是 PR-B 要单独决定的事。
3. **现有 APK 连不上自签 HTTPS**：`poke-dashboard` 既没有 `networkSecurityConfig`，
   也没有自定义 `TrustManager`，走系统信任库；Android 7 以上不信任用户 CA。
   PR-B 必须同时改客户端（NSC + pin 或 `CertificatePinner` 等价物），不能只改 Mac 端。
4. **证书固定可行且成本低**：设备上 SPKI 比对一次约 5 ms，可以作为"只认这一张证书"的主方案，
   比"信任一个 CA"更难被滥用。

## 已知局限

- 只验证了 `127.0.0.1` + `adb reverse`，**没有验证 LAN 与 UDP 发现**（那需要真 Wi-Fi）。
- 只测了 RSA-2048 单证书链，没测 ECDSA、证书轮换、OCSP。
- 设备系统信任库里是否有用户 CA 未确认（`/data/misc/user/0/cacerts-added/` 需要 root）；
  因此负向用例改用"第二套未安装的 CA"来判定，不依赖系统库内容。
