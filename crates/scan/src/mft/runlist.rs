//! Decodificação de *run lists* de atributos não residentes do NTFS.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    /// Cluster lógico inicial no volume; `None` = trecho esparso (sem clusters alocados).
    pub lcn: Option<u64>,
    /// Comprimento em clusters.
    pub len: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub struct RunlistError;

/// Decodifica a run list. Cada entrada: byte de cabeçalho (nibble baixo = bytes do comprimento,
/// alto = bytes do deslocamento), comprimento sem sinal e deslocamento *com sinal* relativo ao
/// LCN anterior. Cabeçalho 0 encerra a lista.
pub fn decode(data: &[u8]) -> Result<Vec<Run>, RunlistError> {
    let mut runs = Vec::new();
    let mut pos = 0usize;
    let mut lcn: i64 = 0;
    while pos < data.len() {
        let header = data[pos];
        if header == 0 {
            break;
        }
        pos += 1;
        let len_size = (header & 0x0F) as usize;
        let off_size = (header >> 4) as usize;
        if len_size == 0 || len_size > 8 || off_size > 8 || pos + len_size + off_size > data.len() {
            return Err(RunlistError);
        }
        let mut len = 0u64;
        for (i, b) in data[pos..pos + len_size].iter().enumerate() {
            len |= u64::from(*b) << (8 * i);
        }
        pos += len_size;
        if off_size == 0 {
            runs.push(Run { lcn: None, len });
        } else {
            let mut delta = 0i64;
            for (i, b) in data[pos..pos + off_size].iter().enumerate() {
                delta |= i64::from(*b) << (8 * i);
            }
            // Extensão de sinal.
            if data[pos + off_size - 1] & 0x80 != 0 && off_size < 8 {
                delta |= -1i64 << (8 * off_size);
            }
            pos += off_size;
            lcn = lcn.checked_add(delta).ok_or(RunlistError)?;
            if lcn < 0 {
                return Err(RunlistError);
            }
            runs.push(Run {
                lcn: Some(lcn as u64),
                len,
            });
        }
    }
    Ok(runs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_and_negative_delta() {
        // 0x21: comprimento 1 byte, deslocamento 2 bytes. 0x18 clusters em LCN 0x1000;
        // depois 0x10 clusters em LCN 0x1000 - 0x100 = 0x0F00.
        let data = [0x21, 0x18, 0x00, 0x10, 0x21, 0x10, 0x00, 0xFF, 0x00];
        let runs = decode(&data).unwrap();
        assert_eq!(
            runs,
            vec![
                Run {
                    lcn: Some(0x1000),
                    len: 0x18
                },
                Run {
                    lcn: Some(0x0F00),
                    len: 0x10
                }
            ]
        );
    }

    #[test]
    fn sparse_run() {
        let data = [0x01, 0x05, 0x11, 0x02, 0x07, 0x00];
        let runs = decode(&data).unwrap();
        assert_eq!(runs[0], Run { lcn: None, len: 5 });
        assert_eq!(
            runs[1],
            Run {
                lcn: Some(7),
                len: 2
            }
        );
    }

    #[test]
    fn rejects_truncated() {
        assert_eq!(decode(&[0x21, 0x18]), Err(RunlistError));
        assert_eq!(
            decode(&[0x01, 0x05, 0x11, 0x02, 0xF0, 0x00]),
            Err(RunlistError)
        );
    }
}
