use super::*;

pub(super) fn materialize_empty_files(
    destination: &Path,
    pending: &mut Vec<(u16, String)>,
    done_ids: &mut Vec<u16>,
    saved: &mut Vec<PathBuf>,
    done: &mut usize,
    clip_info: &mut Option<ClipInfo>,
) {
    for (empty_id, name) in pending.drain(..) {
        if name.trim().is_empty() {
            continue;
        }

        if clip::is_target(&name) {
            *clip_info = Some(clip::deliver(&[], false, 0));
            if !done_ids.contains(&empty_id) {
                done_ids.push(empty_id);
            }
            *done += 1;
            continue;
        }
        match create_part_atomic(destination, &name) {
            Ok((target, part, raw)) => {
                drop(raw);
                if let Err(e) = std::fs::rename(&part, &target) {
                    crate::trace::note("file#1", &format!("空文件改名失败 {name}: {e}"));
                    continue;
                }
                crate::trace::note("file#1", &format!("空文件已创建 {}", target.display()));
                if !done_ids.contains(&empty_id) {
                    done_ids.push(empty_id);
                }
                saved.push(target);
                *done += 1;
            }
            Err(e) => crate::trace::note("file#1", &format!("空文件创建失败 {name}: {e}")),
        }
    }
}

pub(super) struct PartGuard {
    path: Option<PathBuf>,
    keep: bool,
}

impl PartGuard {
    pub(super) fn new() -> Self {
        Self { path: None, keep: false }
    }
    pub(super) fn track(&mut self, p: PathBuf) {
        self.path = Some(p);
    }
    pub(super) fn clear(&mut self) {
        self.path = None;
    }
    pub(super) fn commit(&mut self) {
        self.keep = true;
    }
}

impl Drop for PartGuard {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        if let Some(p) = self.path.take() {
            match std::fs::remove_file(&p) {
                Ok(()) => crate::trace::note("file#1", &format!("已删除未完成文件 {}", p.display())),
                Err(e) => crate::trace::note(
                    "file#1",
                    &format!("删除未完成文件失败 {}：{e}", p.display()),
                ),
            }
        }
    }
}

pub(super) struct OverlayGuard {
    pub(super) done: bool,
}
impl Drop for OverlayGuard {
    fn drop(&mut self) {
        if self.done {
            return;
        }

        ov::abort();
    }
}
