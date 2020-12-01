//! Net document storage backed by sqlite3.
//!
//! We store most objects in sqlite tables, except for very large ones,
//! which we store as "blob" files in a separate directory.

use crate::docmeta::{AuthCertMeta, ConsensusMeta};
use crate::storage::InputString;
use crate::{Error, Result};

use tor_netdoc::doc::authcert::AuthCertKeyIds;
use tor_netdoc::doc::microdesc::MDDigest;

use std::collections::HashMap;
use std::convert::TryInto;
use std::path::{self, Path, PathBuf};
use std::time::SystemTime;

use anyhow::Context;
use chrono::prelude::*;
use chrono::Duration as CDuration;
use rusqlite::{params, OptionalExtension, Transaction, NO_PARAMS};

#[cfg(target_family = "unix")]
use std::os::unix::fs::DirBuilderExt;

/// Local directory cache using a Sqlite3 connection.
pub struct SqliteStore {
    /// Connection to the sqlite3 database.
    conn: rusqlite::Connection,
    /// Location to store blob files.
    path: PathBuf,
}

impl SqliteStore {
    /// Construct a new SquliteStore from a location on disk.  The provided
    /// location must be a directory, or a possible location for a directory:
    /// the directory will be created if necessary.
    pub fn from_path<P>(path: P) -> Result<Self>
    where
        P: AsRef<Path>,
    {
        let path = path.as_ref();
        let sqlpath = path.join("dir.sqlite3");
        let blobpath = path.join("dir_blobs/");

        #[cfg(target_family = "unix")]
        {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&blobpath)
                .with_context(|| format!("Creating directory at {:?}", &blobpath))?;
        }
        #[cfg(not(target_family = "unix"))]
        {
            std::fs::DirBuilder::new()
                .recursive(true)
                .create(&blobpath)
                .with_context(|| format!("Creating directory at {:?}", &blobpath))?;
        }
        let conn = rusqlite::Connection::open(&sqlpath)?;
        SqliteStore::from_conn(conn, &blobpath)
    }

    /// Construct a new SqliteStore from a location on disk, and a location
    /// for blob files.
    pub fn from_conn<P>(conn: rusqlite::Connection, path: P) -> Result<Self>
    where
        P: AsRef<Path>,
    {
        let path = path.as_ref().to_path_buf();
        let mut result = SqliteStore { conn, path };

        result.check_schema()?;

        Ok(result)
    }

    /// Check whether this database has a schema format we can read, and
    /// install or upgrade the schema if necessary.
    fn check_schema(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        let db_n_tables: u32 = tx.query_row(
            "SELECT COUNT(name) FROM sqlite_master
             WHERE type='table'
             AND name NOT LIKE 'sqlite_%'",
            NO_PARAMS,
            |row| row.get(0),
        )?;
        let db_exists = db_n_tables > 0;

        if !db_exists {
            tx.execute_batch(INSTALL_SCHEMA)?;
            tx.commit()?;
            return Ok(());
        }

        let (_version, readable_by): (u32, u32) = tx.query_row(
            "SELECT version, readable_by FROM TorSchemaMeta
             WHERE name = 'TorDirStorage'",
            NO_PARAMS,
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;

        /* if version < SCHEMA_VERSION {
            // Update the schema. XXXX
            tx.commit();
            return Ok(())
        } else */
        if readable_by > SCHEMA_VERSION {
            return Err(Error::UnrecognizedSchema.into());
        }

        // rolls back the transaction, but nothing was done.
        Ok(())
    }

    /// Delete all completely-expired objects from the database.
    ///
    /// This is pretty conservative, and only removes things that are
    /// definitely past their good-by date.
    pub fn expire_all(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        let expired_blobs: Vec<String> = {
            let mut stmt = tx.prepare(FIND_EXPIRED_EXTDOCS)?;
            let names = stmt
                .query_map(NO_PARAMS, |row| row.get::<_, String>(0))?
                .filter_map(std::result::Result::ok)
                .collect();
            names
        };

        tx.execute(DROP_OLD_EXTDOCS, NO_PARAMS)?;
        tx.execute(DROP_OLD_MICRODESCS, NO_PARAMS)?;
        tx.execute(DROP_OLD_AUTHCERTS, NO_PARAMS)?;
        tx.execute(DROP_OLD_CONSENSUSES, NO_PARAMS)?;
        tx.commit()?;
        for name in expired_blobs {
            let fname = self.blob_fname(name);
            if let Ok(fname) = fname {
                let _ignore = std::fs::remove_file(fname);
            }
        }
        Ok(())
    }

    /// Return the correct filename for a given blob, based on the filename
    /// from the ExtDocs table.
    fn blob_fname<P>(&self, path: P) -> Result<PathBuf>
    where
        P: AsRef<Path>,
    {
        let path = path.as_ref();
        if !path
            .components()
            .all(|c| matches!(c, path::Component::Normal(_)))
        {
            return Err(Error::CacheCorruption("Invalid path in database").into());
        }

        let mut result = self.path.clone();
        result.push(path);
        Ok(result)
    }

    /// Read a blob from disk, mmapping it if possible.
    fn read_blob<P>(&self, path: P) -> Result<InputString>
    where
        P: AsRef<Path>,
    {
        let path = path.as_ref();
        let full_path = self.blob_fname(path)?;
        InputString::load(&full_path)
            .with_context(|| format!("Loading blob {:?} from storage at {:?}", path, full_path))
    }

    /// Write a file to disk as a blob, and record it in the ExtDocs table.
    ///
    /// Return a SavedBlobHandle that describes where the blob is, and which
    /// can be used either to commit the blob or delete it.
    fn save_blob_internal(
        &mut self,
        contents: &[u8],
        doctype: &str,
        dtype: &str,
        digest: &[u8],
        expires: DateTime<Utc>,
    ) -> Result<SavedBlobHandle<'_>> {
        let digest = hex::encode(digest);
        let digeststr = format!("{}-{}", dtype, digest);
        let fname = format!("{}:{}", doctype, digeststr);
        let full_path = self.blob_fname(&fname)?;

        let unlinker = Unlinker::new(&full_path);
        std::fs::write(full_path, contents)?;

        let tx = self.conn.unchecked_transaction()?;
        tx.execute(INSERT_EXTDOC, params![digeststr, expires, dtype, fname])?;

        Ok(SavedBlobHandle {
            tx,
            fname,
            digeststr,
            unlinker,
        })
    }

    /// Save a blob to disk and commit it.
    #[cfg(test)]
    fn save_blob(
        &mut self,
        contents: &[u8],
        doctype: &str,
        dtype: &str,
        digest: &[u8],
        expires: DateTime<Utc>,
    ) -> Result<String> {
        let h = self.save_blob_internal(contents, doctype, dtype, digest, expires)?;
        let SavedBlobHandle {
            tx,
            digeststr,
            fname,
            unlinker,
        } = h;
        let _ = digeststr;
        tx.commit()?;
        unlinker.forget();
        Ok(fname)
    }

    /// Write a consensus to disk.
    pub fn store_consensus(
        &mut self,
        cmeta: &ConsensusMeta,
        pending: bool,
        contents: &str,
    ) -> Result<()> {
        let lifetime = cmeta.lifetime();
        let sha3_of_signed = cmeta.sha3_256_of_signed();
        let sha3_of_whole = cmeta.sha3_256_of_whole();
        let valid_after: DateTime<Utc> = lifetime.valid_after().into();
        let fresh_until: DateTime<Utc> = lifetime.fresh_until().into();
        let valid_until: DateTime<Utc> = lifetime.valid_after().into();

        // After a few days have passed, a consensus is no good for
        // anything at all, not even diffs.
        let expires = valid_until + CDuration::days(4);

        let h = self.save_blob_internal(
            contents.as_bytes(),
            "mdcon",
            "sha3-256",
            &sha3_of_whole[..],
            expires,
        )?;
        h.tx.execute(
            INSERT_CONSENSUS,
            params![
                valid_after,
                fresh_until,
                valid_until,
                "microdesc",
                pending,
                hex::encode(&sha3_of_signed),
                h.digeststr
            ],
        )?;
        h.tx.commit()?;
        h.unlinker.forget();
        Ok(())
    }

    /// Return the latest `valid-after` time for any non-pending consensus.
    // TODO: Take a pending argument?
    pub fn latest_consensus_time(&self) -> Result<Option<DateTime<Utc>>> {
        if let Some(va) = self
            .conn
            .query_row(FIND_LATEST_CONSENSUS_TIME, NO_PARAMS, |row| row.get(0))
            .optional()?
        {
            Ok(Some(va))
        } else {
            Ok(None)
        }
    }

    /// Load the latest consensus from disk.  If `pending` is true, we
    /// can fetch a consensus that hasn't got enough microdescs yet.
    /// Otherwise, we only want a consensus where we got full
    /// directory information.
    pub fn latest_consensus(&self, pending: bool) -> Result<Option<InputString>> {
        let rv: Option<(DateTime<Utc>, DateTime<Utc>, String)> = self
            .conn
            .query_row(FIND_CONSENSUS, params![pending], |row| row.try_into())
            .optional()?;

        if let Some((_va, _vu, filename)) = rv {
            // XXXX check va and vu.
            // XXXX Some error cases should also be 'None'
            self.read_blob(filename).map(Option::Some)
        } else {
            Ok(None)
        }
    }

    /// Mark the consensus generated from `cmeta` as no longer pending.
    pub fn mark_consensus_usable(&mut self, cmeta: &ConsensusMeta) -> Result<()> {
        let d = hex::encode(cmeta.sha3_256_of_whole());
        let digest = format!("sha3-256-{}", d);

        let tx = self.conn.transaction()?;
        tx.execute(MARK_CONSENSUS_NON_PENDING, params![digest])?;
        tx.commit()?;

        Ok(())
    }

    /// Save a list of authority certificates to the cache.
    pub fn store_authcerts(&mut self, certs: &[(AuthCertMeta, &str)]) -> Result<()> {
        let tx = self.conn.transaction()?;
        let mut stmt = tx.prepare(INSERT_AUTHCERT)?;
        for (meta, content) in certs {
            let ids = meta.key_ids();
            let id_digest = hex::encode(ids.id_fingerprint.as_bytes());
            let sk_digest = hex::encode(ids.sk_fingerprint.as_bytes());
            let published: DateTime<Utc> = meta.published().into();
            let expires: DateTime<Utc> = meta.expires().into();
            stmt.execute(params![id_digest, sk_digest, published, expires, content])?;
        }
        stmt.finalize()?;
        tx.commit()?;
        Ok(())
    }

    /// Read all of the specified authority certs from the cache.
    pub fn authcerts(&self, certs: &[AuthCertKeyIds]) -> Result<HashMap<AuthCertKeyIds, String>> {
        let mut result = HashMap::new();
        // XXXX Do I need to get a transaction here for performance?
        let mut stmt = self.conn.prepare(FIND_AUTHCERT)?;

        for ids in certs {
            let id_digest = hex::encode(ids.id_fingerprint.as_bytes());
            let sk_digest = hex::encode(ids.sk_fingerprint.as_bytes());
            if let Some(contents) = stmt
                .query_row(params![id_digest, sk_digest], |row| row.get::<_, String>(0))
                .optional()?
            {
                result.insert((*ids).clone(), contents);
            }
        }

        Ok(result)
    }

    /// Read all the microdescriptors listed in `input` from the cache.
    pub fn microdescs<'a, I>(&self, input: I) -> Result<HashMap<MDDigest, String>>
    where
        I: IntoIterator<Item = &'a MDDigest>,
    {
        let mut result = HashMap::new();
        let mut stmt = self.conn.prepare(FIND_MD)?;

        // XXXX Should I speed this up with a transaction, or does it not
        // matter for queries?
        for md_digest in input.into_iter() {
            let h_digest = hex::encode(md_digest);
            if let Some(contents) = stmt
                .query_row(params![h_digest], |row| row.get::<_, String>(0))
                .optional()?
            {
                result.insert(*md_digest, contents);
            }
        }

        Ok(result)
    }

    /// Update the `last-listed` time of every microdescriptor in
    /// `input` to `when` or later.
    pub fn update_microdescs_listed<'a, I>(&mut self, input: I, when: SystemTime) -> Result<()>
    where
        I: IntoIterator<Item = &'a MDDigest>,
    {
        let tx = self.conn.transaction()?;
        let mut stmt = tx.prepare(UPDATE_MD_LISTED)?;
        let when: DateTime<Utc> = when.into();

        for md_digest in input.into_iter() {
            let h_digest = hex::encode(md_digest);
            stmt.execute(params![when, h_digest])?;
        }

        stmt.finalize()?;
        tx.commit()?;
        Ok(())
    }

    /// Store every microdescriptor in `input` into the cache, and say that
    /// it was last listed at `when`.
    pub fn store_microdescs<'a, I>(&mut self, input: I, when: SystemTime) -> Result<()>
    where
        I: IntoIterator<Item = (&'a str, &'a MDDigest)>,
    {
        let when: DateTime<Utc> = when.into();

        let tx = self.conn.transaction()?;
        let mut stmt = tx.prepare(INSERT_MD)?;

        for (content, md_digest) in input.into_iter() {
            let h_digest = hex::encode(md_digest);
            stmt.execute(params![h_digest, when, content])?;
        }
        stmt.finalize()?;
        tx.commit()?;
        Ok(())
    }
}

/// Handle to a blob that we have saved to disk but not yet committed to
/// the database.
struct SavedBlobHandle<'a> {
    /// Transaction we're using to add the blob to the ExtDocs table.
    tx: Transaction<'a>,
    /// Filename for the file, with respect to the the blob directory.
    #[allow(unused)]
    fname: String,
    /// Declared digest string for this blob. Of the format
    /// "digesttype-hexstr".
    digeststr: String,
    /// An 'unlinker' for the blob file.
    unlinker: Unlinker,
}

/// Handle to a file which we might have to delete.
///
/// When this handle is dropped, the file gets deleted, unless you have
/// first called [`Unlinker::forget`].
struct Unlinker {
    /// The location of the file to remove, or None if we shouldn't
    /// remove it.
    p: Option<PathBuf>,
}
impl Unlinker {
    /// Make a new Unlinker for a given filename.
    fn new<P: AsRef<Path>>(p: P) -> Self {
        Unlinker {
            p: Some(p.as_ref().to_path_buf()),
        }
    }
    /// Forget about this unlinker, so that the corresponding file won't
    /// get dropped.
    fn forget(mut self) {
        self.p = None
    }
}
impl Drop for Unlinker {
    fn drop(&mut self) {
        if let Some(p) = self.p.take() {
            let _ignore_err = std::fs::remove_file(p);
        }
    }
}

/// Version number used for this version of the arti cache schema.
const SCHEMA_VERSION: u32 = 0;

/// Set up the tables for the arti cache schema in a sqlite database.
const INSTALL_SCHEMA: &str = "
  -- Helps us version the schema.  The schema here corresponds to a
  -- version number called 'version', and it should be readable by
  -- anybody who is compliant with versions of at least 'readable_by'.
  CREATE TABLE TorSchemaMeta (
     name TEXT NOT NULL PRIMARY KEY,
     version INTEGER NOT NULL,
     readable_by INTEGER NOT NULL
  );

  INSERT INTO TorSchemaMeta (name, version, readable_by) VALUES ( 'TorDirStorage', 0, 0 );

  -- Keeps track of external blobs on disk.
  CREATE TABLE ExtDocs (
    -- Records a digest of the file contents, in the form 'dtype-hexstr'
    digest TEXT PRIMARY KEY NOT NULL,
    -- When was this file created?
    created DATE NOT NULL,
    -- After what time will this file definitely be useless?
    expires DATE NOT NULL,
    -- What is the type of this file? Currently supported are 'mdcon'.
    type TEXT NOT NULL,
    -- Filename for this file within our blob directory.
    filename TEXT NOT NULL
  );

  -- All the microdescriptors we know about.
  CREATE TABLE Microdescs (
    sha256_digest TEXT PRIMARY KEY NOT NULL,
    last_listed DATE NOT NULL,
    contents BLOB NOT NULL
  );

  -- All the authority certificates we know.
  CREATE TABLE Authcerts (
    id_digest TEXT NOT NULL,
    sk_digest TEXT NOT NULL,
    published DATE NOT NULL,
    expires DATE NOT NULL,
    contents BLOB NOT NULL,
    PRIMARY KEY (id_digest, sk_digest)
  );

  -- All the consensuses we're storing.
  CREATE TABLE Consensuses (
    valid_after DATE NOT NULL,
    fresh_until DATE NOT NULL,
    valid_until DATE NOT NULL,
    flavor TEXT NOT NULL,
    pending BOOLEAN NOT NULL,
    sha3_of_signed_part TEXT NOT NULL,
    digest TEXT NOT NULL,
    FOREIGN KEY (digest) REFERENCES ExtDocs (digest) ON DELETE CASCADE
  );
  CREATE INDEX Consensuses_vu on CONSENSUSES(valid_until);

";

/// Query: find the latest-expiring microdesc with a given pending status.
const FIND_CONSENSUS: &str = "
  SELECT valid_after, valid_until, filename
  FROM Consensuses
  INNER JOIN ExtDocs ON ExtDocs.digest = Consensuses.digest
  WHERE pending = ? AND flavor = 'microdesc'
  ORDER BY valid_until DESC
  LIMIT 1;
";

/// Query: Find the valid-after time for the latest-expiring
/// non-pending microdesc consensus.
const FIND_LATEST_CONSENSUS_TIME: &str = "
  SELECT valid_after
  FROM Consensuses
  WHERE pending = 0 AND flavor = 'microdesc'
  ORDER BY valid_until DESC
  LIMIT 1;
";

/// Query: Update the consensus whose digest field is 'digest' to call it
/// no longer pending.
const MARK_CONSENSUS_NON_PENDING: &str = "
  UPDATE Consensuses
  SET pending = 0
  WHERE digest = ?;
";

/// Query: Find the authority certificate with given key digests.
const FIND_AUTHCERT: &str = "
  SELECT contents FROM AuthCerts WHERE id_digest = ? AND sk_digest = ?;
";

/// Query: find the microdescriptor with a given hex-encoded sha256 digest
const FIND_MD: &str = "
  SELECT contents
  FROM Microdescs
  WHERE sha256_digest = ?
";

/// Query: find every ExtDocs member that has expired.
const FIND_EXPIRED_EXTDOCS: &str = "
  SELECT filename FROM Extdocs where expires < datetime('now');
";

/// Query: Add a new entry to ExtDocs.
const INSERT_EXTDOC: &str = "
  INSERT OR REPLACE INTO ExtDocs ( digest, created, expires, type, filename )
  VALUES ( ?, datetime('now'), ?, ?, ? );
";

/// Qury: Add a new consensus.
const INSERT_CONSENSUS: &str = "
  INSERT OR REPLACE INTO Consensuses
    ( valid_after, fresh_until, valid_until, flavor, pending, sha3_of_signed_part, digest )
  VALUES ( ?, ?, ?, ?, ?, ?, ? );
";

/// Query: Add a new AuthCert
const INSERT_AUTHCERT: &str = "
  INSERT OR REPLACE INTO Authcerts
    ( id_digest, sk_digest, published, expires, contents)
  VALUES ( ?, ?, ?, ?, ? );
";

/// Query: Add a new microdescriptor
const INSERT_MD: &str = "
  INSERT OR REPLACE INTO Microdescs ( sha256_digest, last_listed, contents )
  VALUES ( ?, ?, ? );
";

/// Query: Change the time when a given microdescriptor was last listed.
const UPDATE_MD_LISTED: &str = "
  UPDATE Microdescs
  SET last_listed = max(last_listed, ?)
  WHERE sha256_digest = ?;
";

/// Query: Discard every expired extdoc.
const DROP_OLD_EXTDOCS: &str = "
  DELETE FROM ExtDocs WHERE expires < datetime('now');
";

/// Query: Discard every microdescriptor that hasn't been listed for 3 months.
// TODO: Choose a more realistic time.
const DROP_OLD_MICRODESCS: &str = "
  DELETE FROM Microdescs WHERE last_listed < datetime('now','-3 months');
";
/// Query: Discard every expired authority certificate.
const DROP_OLD_AUTHCERTS: &str = "
  DELETE FROM Authcerts WHERE expires < datetime('now');
";
/// Query: Discard every consensus that's been expired for at least
/// two days.
const DROP_OLD_CONSENSUSES: &str = "
  DELETE FROM Consensuses WHERE valid_until < datetime('now','-2 days');
";

#[cfg(test)]
mod test {
    use super::*;
    use hex_literal::hex;
    use tempdir::TempDir;

    fn new_empty() -> Result<(TempDir, SqliteStore)> {
        let tmp_dir = TempDir::new("arti-nd").unwrap();
        let sql_path = tmp_dir.path().join("db.sql");
        let conn = rusqlite::Connection::open(&sql_path)?;
        let store = SqliteStore::from_conn(conn, &tmp_dir)?;

        Ok((tmp_dir, store))
    }

    #[test]
    fn init() -> Result<()> {
        let tmp_dir = TempDir::new("arti-nd").unwrap();
        let sql_path = tmp_dir.path().join("db.sql");
        // Initial setup: everything should work.
        {
            let conn = rusqlite::Connection::open(&sql_path)?;
            let _store = SqliteStore::from_conn(conn, &tmp_dir)?;
        }
        // Second setup: shouldn't need to upgrade.
        {
            let conn = rusqlite::Connection::open(&sql_path)?;
            let _store = SqliteStore::from_conn(conn, &tmp_dir)?;
        }
        // Third setup: shouldn't need to upgrade.
        {
            let conn = rusqlite::Connection::open(&sql_path)?;
            conn.execute_batch("UPDATE TorSchemaMeta SET version = 9002;")?;
            let _store = SqliteStore::from_conn(conn, &tmp_dir)?;
        }
        // Fourth: this says we can't read it, so we'll get an error.
        {
            let conn = rusqlite::Connection::open(&sql_path)?;
            conn.execute_batch("UPDATE TorSchemaMeta SET readable_by = 9001;")?;
            let val = SqliteStore::from_conn(conn, &tmp_dir);
            assert!(val.is_err());
        }
        Ok(())
    }

    #[test]
    fn bad_blob_fnames() -> Result<()> {
        let (_tmp_dir, store) = new_empty()?;

        assert!(store.blob_fname("abcd").is_ok());
        assert!(store.blob_fname("abcd..").is_ok());
        assert!(store.blob_fname("..abcd..").is_ok());
        assert!(store.blob_fname(".abcd").is_ok());

        assert!(store.blob_fname(".").is_err());
        assert!(store.blob_fname("..").is_err());
        assert!(store.blob_fname("../abcd").is_err());
        assert!(store.blob_fname("/abcd").is_err());

        Ok(())
    }

    #[test]
    fn blobs() -> Result<()> {
        let (tmp_dir, mut store) = new_empty()?;

        let now = Utc::now();
        let one_week = CDuration::weeks(1);

        let fname1 = store.save_blob(
            b"Hello world",
            "greeting",
            "sha1",
            &hex!("7b502c3a1f48c8609ae212cdfb639dee39673f5e"),
            now + one_week,
        )?;

        let fname2 = store.save_blob(
            b"Goodbye, dear friends",
            "greeting",
            "sha1",
            &hex!("2149c2a7dbf5be2bb36fb3c5080d0fb14cb3355c"),
            now - one_week,
        )?;

        assert_eq!(
            fname1,
            "greeting:sha1-7b502c3a1f48c8609ae212cdfb639dee39673f5e"
        );
        assert_eq!(store.blob_fname(&fname1)?, tmp_dir.path().join(&fname1));
        assert_eq!(
            &std::fs::read(store.blob_fname(&fname1)?)?[..],
            b"Hello world"
        );
        assert_eq!(
            &std::fs::read(store.blob_fname(&fname2)?)?[..],
            b"Goodbye, dear friends"
        );

        let n: u32 =
            store
                .conn
                .query_row("SELECT COUNT(filename) FROM ExtDocs", NO_PARAMS, |row| {
                    row.get(0)
                })?;
        assert_eq!(n, 2);

        let blob = store.read_blob(&fname2)?;
        assert_eq!(blob.as_str().unwrap(), "Goodbye, dear friends");

        // Now expire: the second file should go away.
        store.expire_all()?;
        assert_eq!(
            &std::fs::read(store.blob_fname(&fname1)?)?[..],
            b"Hello world"
        );
        assert!(std::fs::read(store.blob_fname(&fname2)?).is_err());
        let n: u32 =
            store
                .conn
                .query_row("SELECT COUNT(filename) FROM ExtDocs", NO_PARAMS, |row| {
                    row.get(0)
                })?;
        assert_eq!(n, 1);

        Ok(())
    }

    #[test]
    fn consensus() -> Result<()> {
        use tor_netdoc::doc::netstatus;

        let (_tmp_dir, mut store) = new_empty()?;
        let now = Utc::now();
        let one_hour = CDuration::hours(1);

        assert_eq!(store.latest_consensus_time()?, None);

        let cmeta = ConsensusMeta::new(
            netstatus::Lifetime::new(
                now.into(),
                (now + one_hour).into(),
                (now + one_hour * 2).into(),
            ),
            [0xAB; 32],
            [0xBC; 32],
        );

        store.store_consensus(&cmeta, true, "Pretend this is a consensus")?;

        {
            assert_eq!(store.latest_consensus_time()?, None);
            let consensus = store.latest_consensus(true)?.unwrap();
            assert_eq!(consensus.as_str()?, "Pretend this is a consensus");
        }

        store.mark_consensus_usable(&cmeta)?;

        {
            assert_eq!(store.latest_consensus_time()?, now.into());
            let consensus = store.latest_consensus(true)?;
            assert!(consensus.is_none());
            let consensus = store.latest_consensus(false)?.unwrap();
            assert_eq!(consensus.as_str()?, "Pretend this is a consensus");
        }
        Ok(())
    }

    #[test]
    fn authcerts() -> Result<()> {
        let (_tmp_dir, mut store) = new_empty()?;
        let now = Utc::now();
        let one_hour = CDuration::hours(1);

        let keyids = AuthCertKeyIds {
            id_fingerprint: [3; 20].into(),
            sk_fingerprint: [4; 20].into(),
        };
        let keyids2 = AuthCertKeyIds {
            id_fingerprint: [4; 20].into(),
            sk_fingerprint: [3; 20].into(),
        };

        let m1 = AuthCertMeta::new(keyids.clone(), now.into(), (now + one_hour * 24).into());

        store.store_authcerts(&[(m1, "Pretend this is a cert")])?;

        let certs = store.authcerts(&[keyids.clone(), keyids2])?;
        assert_eq!(certs.len(), 1);
        assert_eq!(certs.get(&keyids).unwrap(), "Pretend this is a cert");

        Ok(())
    }

    #[test]
    fn microdescs() -> Result<()> {
        let (_tmp_dir, mut store) = new_empty()?;

        let now = Utc::now();
        let one_day = CDuration::days(1);

        let d1 = [5_u8; 32];
        let d2 = [7; 32];
        let d3 = [42; 32];
        let d4 = [99; 32];

        store.store_microdescs(
            vec![
                ("Fake micro 1", &d1),
                ("Fake micro 2", &d2),
                ("Fake micro 3", &d3),
            ],
            (now - one_day * 100).into(),
        )?;

        store.update_microdescs_listed(&[d2], now.into())?;

        let mds = store.microdescs(&[d2, d3, d4])?;
        assert_eq!(mds.len(), 2);
        assert_eq!(mds.get(&d1), None);
        assert_eq!(mds.get(&d2).unwrap(), "Fake micro 2");
        assert_eq!(mds.get(&d3).unwrap(), "Fake micro 3");
        assert_eq!(mds.get(&d4), None);

        // Now we'll expire.  that should drop everything but d2.
        store.expire_all()?;
        let mds = store.microdescs(&[d2, d3, d4])?;
        assert_eq!(mds.len(), 1);
        assert_eq!(mds.get(&d2).unwrap(), "Fake micro 2");

        Ok(())
    }
}