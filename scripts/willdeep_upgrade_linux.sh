#!/usr/bin/env bash
# willdeep-upgrade -- 在本机安装指定的 WillDeep Release，并重启 systemd 守护进程。
#
# 为什么重启用 systemctl restart 而不是 willdeep daemon upgrade:
# 在 systemd 下 daemon upgrade 会让旧主进程退出，systemd 据此判定单元已结束
# （inactive/SubState=dead），交接后的新进程与单元脱钩，systemctl start 也接不回来。
# 因此升级统一走 restart: ExecStop 用 daemon stop 干净收尾，ExecStart 起新版本。
# 代价是活跃任务会被取消，脚本因此默认检测并拒绝在有活跃任务时重启（--force 覆盖）。
set -euo pipefail

SERVICE="willdeep-daemon"
RUN_USER="willdeep"
RUN_HOME="/home/willdeep"
WILLDEEP_BIN="$RUN_HOME/.local/bin/willdeep"
LINK_BIN="/usr/local/bin/willdeep"
BACKUP_DIR="/var/backups/willdeep"
REPO="niuwoai/willdeep-rs"
KEEP_BACKUPS=3
# 国内直连 GitHub Release 约 20KB/s，镜像优先，最后退回直连。
MIRROR_PREFIXES="${WILLDEEP_MIRROR_PREFIXES:-https://ghfast.top/ https://gh-proxy.com/ }"

target_tag=""
check_only=0
do_restart=1
force=0
verify=1
if [ "${WILLDEEP_NO_VERIFY:-0}" = "1" ]; then verify=0; fi

die() { echo "willdeep-upgrade: $*" >&2; exit 1; }
log() { echo "==> $*"; }

usage() {
  cat >&2 <<USAGE
用法: willdeep-upgrade [选项]

  --version <tag>   指定 Release tag，例如 v0.96.0-rc1（默认取 latest）
  --check           只显示当前与目标版本，不做任何改动
  --no-restart      只替换二进制，不重启 $SERVICE
  --force           同版本重装、或存在活跃任务时也继续
  --help            显示本帮助

流程: 解析版本 -> 下载 willdeep-linux-<arch>.tar.gz -> 跨镜像 sha256 交叉校验
      -> 检查活跃任务 -> 备份旧二进制 -> 原子替换 -> 重启 $SERVICE -> 校验运行版本
      -> 重启失败或版本不符时自动回滚到备份并重启

环境变量:
  WILLDEEP_MIRROR_PREFIXES  镜像前缀，空格分隔，默认 "$MIRROR_PREFIXES"
  WILLDEEP_NO_VERIFY=1      跳过跨镜像交叉校验

注意: 重启走 willdeep daemon stop，活跃任务会被取消；默认检测到活跃任务即拒绝，
      且这个检查发生在动任何文件之前。
USAGE
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version) [ "$#" -ge 2 ] || die "--version 缺少参数"; target_tag="$2"; shift 2 ;;
    --check) check_only=1; shift ;;
    --no-restart) do_restart=0; shift ;;
    --force) force=1; shift ;;
    --help|-h) usage; exit 0 ;;
    *) die "未知参数: $1（--help 查看用法）" ;;
  esac
done

[ "$(id -u)" -eq 0 ] || die "需要 root 权限，请用 sudo willdeep-upgrade"
case "$(uname -s)" in Linux) ;; *) die "本脚本只支持 Linux" ;; esac
case "$(uname -m)" in
  x86_64|amd64) arch="amd64" ;;
  aarch64|arm64) arch="arm64" ;;
  *) die "不支持的架构: $(uname -m)" ;;
esac

as_run_user() {
  runuser -u "$RUN_USER" -- env HOME="$RUN_HOME" WILLDEEP_HOME="$RUN_HOME/.willdeep" "$@"
}

current_version="-"
if [ -x "$WILLDEEP_BIN" ]; then
  current_version="$(HOME=/nonexistent timeout 30 "$WILLDEEP_BIN" --version 2>/dev/null | cut -d" " -f2)" || true
  [ -n "$current_version" ] || current_version="-"
fi

if [ -z "$target_tag" ]; then
  log "解析最新 Release"
  target_tag="$(curl -fsSL --connect-timeout 15 --max-time 60 "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null | sed -n "s/.*\"tag_name\": *\"\([^\"]*\)\".*/\1/p" | head -n1)" || true
  if [ -z "$target_tag" ]; then
    target_tag="$(curl -fsSI --connect-timeout 15 --max-time 60 -o /dev/null -w "%{redirect_url}" "https://github.com/$REPO/releases/latest" 2>/dev/null | sed -n "s#.*/tag/##p")" || true
  fi
  [ -n "$target_tag" ] || die "拿不到最新版本号，请用 --version vX.Y.Z 指定"
fi
target_version="${target_tag#v}"

log "当前版本: $current_version"
log "目标版本: $target_tag  (linux-$arch)"
if [ "$check_only" = "1" ]; then exit 0; fi
if [ "$current_version" = "$target_version" ] && [ "$force" != "1" ]; then
  log "已经是 ${target_version}，无需升级；要强制重装加 --force"
  exit 0
fi

workdir="$(mktemp -d)"
cleanup() { rm -rf "$workdir"; }
trap cleanup EXIT

asset="willdeep-linux-$arch.tar.gz"
url="https://github.com/$REPO/releases/download/$target_tag/$asset"

download() {
  curl -fL --connect-timeout 20 --max-time 1800 --retry 2 --retry-delay 3 -o "$2" "$1"
}

log "下载 $asset"
got=0
first_prefix=""
for prefix in $MIRROR_PREFIXES ""; do
  label="${prefix:-直连 github.com}"
  if download "${prefix}${url}" "$workdir/pkg.tar.gz" 2>/dev/null; then
    log "  成功: $label"
    got=1
    first_prefix="$prefix"
    break
  fi
  log "  失败: $label"
done
[ "$got" = "1" ] || die "所有镜像都下载失败，可用 WILLDEEP_MIRROR_PREFIXES 覆盖"

primary_sha="$(sha256sum "$workdir/pkg.tar.gz" | cut -d" " -f1)"
log "sha256: $primary_sha"
if [ "$verify" = "1" ]; then
  checked=0
  for prefix in $MIRROR_PREFIXES ""; do
    [ "$prefix" = "$first_prefix" ] && continue
    if download "${prefix}${url}" "$workdir/pkg-alt.tar.gz" 2>/dev/null; then
      alt_sha="$(sha256sum "$workdir/pkg-alt.tar.gz" | cut -d" " -f1)"
      [ "$alt_sha" = "$primary_sha" ] || die "镜像 ${prefix:-直连 github.com} 的 sha256 与主下载不一致，已中止"
      log "  交叉校验通过: ${prefix:-直连 github.com}"
      checked=1
      rm -f "$workdir/pkg-alt.tar.gz"
      break
    fi
  done
  if [ "$checked" != "1" ]; then log "  警告: 没有第二个镜像可用，跳过交叉校验"; fi
fi

extract="$workdir/extract"
mkdir -p "$extract"
tar -xzf "$workdir/pkg.tar.gz" --no-same-owner -C "$extract" || die "解包失败"
[ -f "$extract/willdeep" ] || die "包内没有 willdeep"
chmod 0755 "$extract/willdeep"
new_version="$(HOME=/nonexistent timeout 30 "$extract/willdeep" --version 2>/dev/null | cut -d" " -f2)" || true
[ -n "$new_version" ] || die "新二进制 --version 没有输出"
[ "$new_version" = "$target_version" ] || die "版本不符: 包内 ${new_version}，期望 ${target_version}"
log "包内版本: $new_version"

if [ "$do_restart" = "1" ]; then
  active="$(as_run_user timeout 30 "$WILLDEEP_BIN" daemon tasks 2>/dev/null || true)"
  if [ -n "$active" ] && printf "%s" "$active" | grep -Eqi "running|queued|waiting"; then
    printf "%s\n" "$active"
    [ "$force" = "1" ] || die "有活跃任务，重启会取消它们；确认后加 --force（此时尚未改动任何文件）"
    log "有活跃任务，--force 已指定，继续"
  fi
fi

backup=""
if [ -f "$WILLDEEP_BIN" ]; then
  install -d -m 0755 "$BACKUP_DIR"
  stamp="$(date +%Y%m%d-%H%M%S)"
  backup="$BACKUP_DIR/willdeep-$current_version-$stamp"
  cp -p "$WILLDEEP_BIN" "$backup"
  log "已备份: $backup"
  ls -1t "$BACKUP_DIR"/willdeep-* 2>/dev/null | tail -n "+$((KEEP_BACKUPS + 1))" | xargs -r rm -f || true
fi

install -m 0755 -o "$RUN_USER" -g "$RUN_USER" "$extract/willdeep" "$RUN_HOME/.local/bin/.willdeep.new"
mv -f "$RUN_HOME/.local/bin/.willdeep.new" "$WILLDEEP_BIN"
[ -e "$LINK_BIN" ] || ln -sf "$WILLDEEP_BIN" "$LINK_BIN"
log "二进制已更新: $WILLDEEP_BIN"

if [ "$do_restart" != "1" ]; then
  log "按 --no-restart 跳过重启；守护进程仍跑 ${current_version}，之后执行: systemctl restart $SERVICE"
  exit 0
fi

rollback() {
  if [ -n "$backup" ] && [ -f "$backup" ]; then
    log "回滚到 $current_version"
    install -m 0755 -o "$RUN_USER" -g "$RUN_USER" "$backup" "$RUN_HOME/.local/bin/.willdeep.rollback"
    mv -f "$RUN_HOME/.local/bin/.willdeep.rollback" "$WILLDEEP_BIN"
    systemctl restart "$SERVICE" || true
  else
    log "没有可用备份，跳过回滚"
  fi
}

log "重启 $SERVICE"
if ! systemctl restart "$SERVICE"; then
  log "重启失败"
  rollback
  die "重启 $SERVICE 失败，详见 journalctl -u $SERVICE"
fi

deadline=$((SECONDS + 90))
running_version=""
while [ "$SECONDS" -lt "$deadline" ]; do
  if systemctl is-active --quiet "$SERVICE"; then
    status="$(as_run_user timeout 30 "$WILLDEEP_BIN" daemon status 2>/dev/null || true)"
    running_version="$(printf "%s" "$status" | sed -n "s/.*version=\([^[:space:]]*\).*/\1/p")"
    if [ "$running_version" = "$new_version" ]; then break; fi
  fi
  sleep 1
done

if [ "$running_version" != "$new_version" ]; then
  log "校验失败: 运行中的版本是 ${running_version:-未知}，期望 $new_version"
  rollback
  die "升级未生效，详见 journalctl -u $SERVICE"
fi

log "升级完成: $current_version -> $running_version"
printf "%s\n" "$status"
