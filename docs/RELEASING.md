# 发行与固定签名

推送形如 `v0.1.0` 的 tag 会触发 `.github/workflows/release.yml`。工作流先要求 tag、`package.json`、`src-tauri/tauri.conf.json` 和 `src-tauri/Cargo.toml` 的版本完全一致，再跑前端和 Rust 质量门禁，最后生成：

- 通用 macOS DMG：Apple Developer ID 签名、Apple 公证和 stapling；
- Windows x64 MSI 与 NSIS 安装程序：固定 Authenticode 证书签名并时间戳；
- Android 通用 APK 与 AAB：固定 upload keystore 签名。

三种签名的证书指纹均由 GitHub repository variable 固定。工作流会在构建前比对指纹，并在构建后再次验证产物签名；缺失、变更或错误的签名材料会让工作流失败，不会上传未签名发行包。所有私钥只在 runner 的临时目录中解码，完成后删除；仓库不会保存证书、PFX、JKS、`.p8` 或密码。

## GitHub 配置

在仓库 Settings → Secrets and variables → Actions 中配置以下内容。

Secrets：

| 名称 | 内容 |
| --- | --- |
| `APPLE_CERTIFICATE` | Developer ID Application `.p12` 的单行 base64 |
| `APPLE_CERTIFICATE_PASSWORD` | 导出 `.p12` 时设置的密码 |
| `APPLE_API_KEY` | App Store Connect API Key ID |
| `APPLE_API_ISSUER` | App Store Connect Issuer ID |
| `APPLE_API_KEY_BASE64` | App Store Connect `.p8` 私钥的单行 base64 |
| `WINDOWS_CERTIFICATE_PFX_BASE64` | 固定 Authenticode `.pfx` 的单行 base64 |
| `WINDOWS_CERTIFICATE_PASSWORD` | 导出 `.pfx` 时设置的密码 |
| `ANDROID_KEYSTORE_BASE64` | 固定 Android upload `.jks` 的单行 base64 |
| `ANDROID_KEYSTORE_PASSWORD` | JKS store password |
| `ANDROID_KEY_ALIAS` | JKS 中发布密钥的 alias |
| `ANDROID_KEY_PASSWORD` | 该 alias 的 key password |

Repository variables：

| 名称 | 内容 |
| --- | --- |
| `MACOS_SIGNING_IDENTITY` | 完整 Developer ID identity，例如 `Developer ID Application: Example, Inc. (TEAMID)` |
| `MACOS_CERTIFICATE_SHA1` | `.p12` 叶证书的 SHA-1 指纹，可带或不带冒号 |
| `WINDOWS_CERTIFICATE_SHA1` | `.pfx` 签名证书的 SHA-1 thumbprint，可带或不带冒号 |
| `ANDROID_CERTIFICATE_SHA256` | Android upload 证书的 SHA-256 指纹，可带或不带冒号 |

不要轮换任一发行私钥。若证书确实必须更新，先在所有安装渠道协调迁移，再更新对应 secret 和固定指纹 variable；否则新的包会被工作流明确拒绝。

## 生成配置值

在安全的本机环境中执行，得到的文件和输出不要提交到仓库：

```bash
# macOS：将导出的 Developer ID .p12 和 App Store Connect .p8 转成单行 base64
openssl base64 -A -in DeveloperID.p12 -out apple-certificate.base64
openssl base64 -A -in AuthKey_ABC123.p8 -out apple-api-key.base64
openssl pkcs12 -in DeveloperID.p12 -clcerts -nokeys -passin pass:YOUR_PASSWORD \
  | openssl x509 -noout -fingerprint -sha1

# Windows：将固定 Authenticode PFX 转成单行 base64，再在 Windows 上读取 thumbprint
openssl base64 -A -in MutsumiMail.pfx -out windows-certificate.base64
Get-PfxCertificate .\\MutsumiMail.pfx | Select-Object -ExpandProperty Thumbprint

# Android：只创建一次 upload key；随后复用这个 JKS
keytool -genkeypair -v -keystore mutsumi-mail-upload.jks -alias mutsumi-mail-upload \
  -keyalg RSA -keysize 4096 -validity 10000
openssl base64 -A -in mutsumi-mail-upload.jks -out android-keystore.base64
keytool -list -v -keystore mutsumi-mail-upload.jks -alias mutsumi-mail-upload
```

`APPLE_CERTIFICATE` 必须是能用于外部分发的 Developer ID Application 证书；自签名 macOS 证书和临时 Windows PFX 不会获得 Gatekeeper 或 SmartScreen 信任，不能替代正式发行身份。Android 的首次 AAB 上传仍需在 Play Console 手动完成，以建立该 upload key 的信任关系。

## 发布

先更新三个版本文件为同一个版本，核对后再创建 tag：

```bash
pnpm release:verify-version -- v0.1.0
git tag -a v0.1.0 -m "Mutsumi Mail 0.1.0"
git push origin v0.1.0
```

成功后工作流会创建或更新同名 GitHub Release，并上传 DMG、MSI、NSIS EXE、APK、AAB 和 `SHA256SUMS.txt`。没有配置完上述 secrets/variables 时，不要推送发行 tag；工作流会有意在签名输入检查阶段失败，而不会生成 unsigned fallback。
