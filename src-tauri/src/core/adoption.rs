use super::*;
use managed::FailureKind;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Adoption {
    Applied,
    AlreadyCurrent,
    Deferred,
}

impl CoreService {
    pub fn try_adopt(&self) -> Result<Adoption, String> {
        if self.gate.core_fetching() || self.gate.runtime_busy() || self.gate.closing() {
            return Ok(Adoption::Deferred);
        }
        let op = self.gate.begin(Kind::Selection)?;
        let was_usable = self.usable.load(Ordering::Acquire);
        let result = self.adopt_owned(&op);
        drop(op);
        if !was_usable && self.usable.load(Ordering::Acquire) {
            self.wake.notify_one();
        }
        result
    }

    fn adoption_allowed(&self, op: &Operation, files: &Operation, p: &Prepared) -> bool {
        let s = self.store.snapshot();
        !op.cancel.load(Ordering::Acquire)
            && !files.cancel.load(Ordering::Acquire)
            && !self.gate.closing()
            && !self.managed.suppressed.load(Ordering::Acquire)
            && p.generation == self.generation()
            && (s.managed() || *self.takeover.lock().unwrap() == Some(p.generation))
            && (p.origin != Origin::Automatic || s.update_preferences.automatic_core)
    }

    pub(crate) fn adopt_owned(&self, op: &Operation) -> Result<Adoption, String> {
        let Ok(files) = self.gate.begin(Kind::CoreFetch) else {
            return Ok(Adoption::Deferred);
        };
        let Some(p) = self.managed.ready() else {
            return Ok(Adoption::Deferred);
        };
        if !self.adoption_allowed(op, &files, &p) {
            self.managed.update(|v| v.phase = Phase::Paused);
            return Ok(Adoption::Deferred);
        }
        let result = self.adopt_prepared(op, &files, p.clone());
        match result {
            Ok(Adoption::Deferred) => {
                let allowed = self.adoption_allowed(op, &files, &p);
                self.managed.update(|v| {
                    if v.phase == Phase::Preparing {
                        v.phase = if allowed { Phase::Ready } else { Phase::Paused };
                    }
                });
                Ok(Adoption::Deferred)
            }
            Ok(outcome) => Ok(outcome),
            Err(_) if !self.adoption_allowed(op, &files, &p) => {
                self.managed.update(|v| v.phase = Phase::Paused);
                Ok(Adoption::Deferred)
            }
            Err((kind, error)) => {
                self.managed.suppressed.store(true, Ordering::Release);
                self.managed.fail(kind, error.clone());
                Err(error)
            }
        }
    }

    fn adopt_prepared(
        &self,
        op: &Operation,
        files: &Operation,
        mut p: Prepared,
    ) -> Result<Adoption, (FailureKind, String)> {
        let local = |e| (FailureKind::Local, e);
        let conflict = |e: &str| (FailureKind::Conflict, e.to_string());
        let s = self.store.snapshot();
        let source = p
            .reference
            .source
            .as_ref()
            .ok_or_else(|| conflict("准备记录缺少来源"))?;
        if s.managed
            .blocked
            .as_ref()
            .is_some_and(|b| same_asset(&b.candidate, source))
            && p.origin != Origin::Manual
        {
            return Ok(Adoption::Deferred);
        }
        if self.runtime.stopped().is_err() {
            return Ok(Adoption::Deferred);
        }
        self.managed.update(|v| v.phase = Phase::Preparing);
        if let Some(active) = &s.managed.active {
            if active.version == p.reference.version && self.verify(active, &op.cancel).is_ok() {
                if active.hash != p.reference.hash
                    || active
                        .source
                        .as_ref()
                        .is_some_and(|c| !same_asset(c, source))
                {
                    return Err(conflict("已选版本的官方资产发生变化，保留现有核心"));
                }
                self.usable.store(true, Ordering::Release);
                self.managed.consume_ready();
                return Ok(Adoption::AlreadyCurrent);
            }
            if s.managed.trial.is_some() && self.usable.load(Ordering::Acquire) {
                return Ok(Adoption::Deferred);
            }
            if settings::stable(&p.reference.version).map_err(local)?
                < settings::stable(&active.version).map_err(local)?
            {
                self.managed.clear_ready().map_err(local)?;
                return Ok(Adoption::Deferred);
            }
        }
        let program = self
            .managed
            .restore_program(&mut p, &op.cancel)
            .map_err(|e| (FailureKind::Cache, e))?;
        self.managed.save_prepared(p.clone()).map_err(local)?;
        if s.profile.mode == crate::profile::Mode::Generated
            && crate::profile::tun::require_version(&p.reference.version).is_err()
        {
            let error = "此核心版本尚未验证受控 TUN，保留现有版本".to_string();
            self.block(&p, FailureStage::Compatibility, error.clone())
                .map_err(local)?;
            return Err((FailureKind::Compatibility, error));
        }
        let config = self.config_for(&s).map_err(local)?;
        let mut config_hash = None;
        if s.managed.active.is_some() || !s.managed() {
            if let Some(config) = &config {
                let hash = package::hash_cancel(config, &op.cancel).map_err(local)?;
                if let Err(error) = self.runtime.validate(&program, config, &op.cancel) {
                    if self.adoption_allowed(op, files, &p) {
                        self.block(&p, FailureStage::Compatibility, error.clone())
                            .map_err(local)?;
                    }
                    return Err((
                        FailureKind::Compatibility,
                        format!("新版暂不兼容当前配置，保留原核心：{error}"),
                    ));
                }
                config_hash = Some(hash);
            }
        }
        self.adoption_checkpoint("before_publish");
        if !self.adoption_allowed(op, files, &p) {
            return Ok(Adoption::Deferred);
        }
        let installed = settings::managed_path(&self.store.root, &p.reference).map_err(local)?;
        let staged = package::staged_path(&self.store.root, &p.reference).map_err(local)?;
        package::owned(&self.store.root, &installed).map_err(|e| (FailureKind::Conflict, e))?;
        package::owned(&self.store.root, &staged).map_err(|e| (FailureKind::Conflict, e))?;
        let previous = s
            .managed
            .active
            .clone()
            .filter(|r| r.version != p.reference.version && self.verify(r, &op.cancel).is_ok())
            .or_else(|| {
                s.managed.previous.clone().filter(|r| {
                    r.version != p.reference.version && self.verify(r, &op.cancel).is_ok()
                })
            });
        if program == staged {
            if installed.exists() {
                if !installed.is_file() {
                    return Err(conflict("核心目标不是普通文件，请检查受管目录"));
                }
                if package::hash_cancel(&installed, &op.cancel).map_err(local)?
                    == *p
                        .reference
                        .hash
                        .as_ref()
                        .ok_or_else(|| conflict("准备记录缺少程序摘要"))?
                {
                    fs::remove_file(&staged).map_err(|e| local(e.to_string()))?;
                } else {
                    let repair = s.managed.active.as_ref().is_some_and(|old| {
                        old.version == p.reference.version
                            && old
                                .source
                                .as_ref()
                                .is_none_or(|c| same_asset(c, p.reference.source.as_ref().unwrap()))
                            && self.verify(old, &op.cancel).is_err()
                    });
                    let referenced = [&s.managed.active, &s.managed.previous]
                        .into_iter()
                        .flatten()
                        .any(|r| r.version == p.reference.version);
                    let external = match &s.core_source {
                        CoreSource::External { path } => {
                            fs::canonicalize(path).ok() == fs::canonicalize(&installed).ok()
                        }
                        _ => false,
                    };
                    if external || (!repair && (referenced || p.origin != Origin::Manual)) {
                        return Err(conflict("受管目录已有不同程序；未登记文件可明确重试准备，有效选择和外部程序不会被覆盖"));
                    }
                    fs::rename(&staged, &installed).map_err(|e| local(e.to_string()))?;
                }
            } else {
                fs::rename(&staged, &installed).map_err(|e| local(e.to_string()))?;
            }
        }
        self.adoption_checkpoint("after_publish");
        if let (Some(config), Some(hash)) = (&config, config_hash) {
            if package::hash_cancel(config, &op.cancel).map_err(local)? != hash {
                return Err(conflict("配置文件已改变，请重新校验"));
            }
        }
        self.adoption_checkpoint("before_commit");
        self.store
            .core(s.core_revision, |v| {
                if !self.adoption_allowed(op, files, &p)
                    || v.nodes_revision != s.nodes_revision
                    || v.routing_revision != s.routing_revision
                {
                    return Err("采用条件已改变，请重新校验".into());
                }
                v.core_source = CoreSource::Managed;
                v.managed.previous = previous;
                v.managed.active = Some(p.reference.clone());
                v.managed.trial = Some(Trial::AwaitingStart);
                v.managed.blocked = None;
                Ok(())
            })
            .map_err(local)?;
        self.adoption_checkpoint("after_commit");
        *self.takeover.lock().unwrap() = None;
        self.usable.store(true, Ordering::Release);
        self.managed.consume_ready();
        self.notify(format!(
            "sing-box {} 已就绪{}",
            p.reference.version,
            if s.profile.mode == crate::profile::Mode::Generated {
                "，首次连接需系统授权"
            } else {
                ""
            }
        ));
        if let Err(e) = self.validate_pending(&op.cancel) {
            *self.config_error.lock().unwrap() = Some(e);
        }
        self.cleanup(&op.cancel);
        Ok(Adoption::Applied)
    }

    fn adoption_checkpoint(&self, stage: &str) {
        #[cfg(feature = "test-fixtures")]
        {
            let hook = self.adoption_hook.lock().unwrap().clone();
            if let Some(hook) = hook {
                hook(stage);
            }
        }
        let _ = stage;
    }
}
