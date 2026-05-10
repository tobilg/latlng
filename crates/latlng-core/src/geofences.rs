use super::*;

impl<P: Platform, S: StorageBackend> LatLng<P, S> {
    pub fn setchan(&self, name: &str, def: GeofenceDef) -> Result<()> {
        let _gate = self.write_control();
        self.ensure_writable()?;
        self.append_log_record(LogRecord::Command(Command::SetChannel {
            name: name.to_owned(),
            def: def.clone(),
        }))?;
        P::write(&self.geofences).set_channel(name.to_owned(), def);
        Ok(())
    }

    pub fn delchan(&self, name: &str) -> Result<bool> {
        let _gate = self.write_control();
        self.ensure_writable()?;
        self.append_log_record(LogRecord::Command(Command::DelChannel {
            name: name.to_owned(),
        }))?;
        Ok(P::write(&self.geofences).del_channel(name))
    }

    pub fn pdelchan(&self, pattern: &str) -> Result<u64> {
        let _gate = self.write_control();
        self.ensure_writable()?;
        self.append_log_record(LogRecord::Command(Command::PDelChannel {
            pattern: pattern.to_owned(),
        }))?;
        Ok(P::write(&self.geofences).pdel_channel(pattern))
    }

    pub fn chans(&self, pattern: &str) -> Result<Vec<String>> {
        let _gate = self.read_control();
        Ok(P::read(&self.geofences).channels(pattern))
    }

    pub fn subscribe(&self, channels: &[&str]) -> GeofenceEventReceiver<P> {
        let _gate = self.read_control();
        P::write(&self.geofences).subscribe(channels)
    }

    pub fn psubscribe(&self, patterns: &[&str]) -> GeofenceEventReceiver<P> {
        let _gate = self.read_control();
        P::write(&self.geofences).psubscribe(patterns)
    }

    pub fn sethook(&self, name: &str, endpoint: &str, def: GeofenceDef) -> Result<()> {
        let _gate = self.write_control();
        self.ensure_writable()?;
        self.append_log_record(LogRecord::Command(Command::SetHook {
            name: name.to_owned(),
            endpoint: endpoint.to_owned(),
            def: def.clone(),
        }))?;
        P::write(&self.geofences).set_hook(name.to_owned(), endpoint.to_owned(), def);
        Ok(())
    }

    pub fn delhook(&self, name: &str) -> Result<bool> {
        let _gate = self.write_control();
        self.ensure_writable()?;
        self.append_log_record(LogRecord::Command(Command::DelHook {
            name: name.to_owned(),
        }))?;
        Ok(P::write(&self.geofences).del_hook(name))
    }

    pub fn pdelhook(&self, pattern: &str) -> Result<u64> {
        let _gate = self.write_control();
        self.ensure_writable()?;
        self.append_log_record(LogRecord::Command(Command::PDelHook {
            pattern: pattern.to_owned(),
        }))?;
        Ok(P::write(&self.geofences).pdel_hook(pattern))
    }

    pub fn hooks(&self, pattern: &str) -> Result<Vec<HookInfo>> {
        let _gate = self.read_control();
        Ok(P::read(&self.geofences).hooks(pattern))
    }

    pub fn channel_defs(&self) -> Vec<ChannelDef> {
        P::read(&self.geofences).channel_defs()
    }

    pub fn channel_def(&self, name: &str) -> Option<ChannelDef> {
        P::read(&self.geofences).channel_def(name)
    }

    pub fn hook_defs(&self) -> Vec<HookDef> {
        P::read(&self.geofences).hook_defs()
    }

    pub fn hook_def(&self, name: &str) -> Option<HookDef> {
        P::read(&self.geofences).hook_def(name)
    }

    pub fn config(&self) -> &P::RwLock<Config> {
        &self.config
    }

    pub fn close_storage(&self) -> Result<()> {
        self.storage.close().map_err(CoreError::from)
    }
}
